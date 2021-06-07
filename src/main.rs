// We want to be able to run
//
//     $ cargo rtic-trace --bin blinky --serial /dev/ttyUSB3
//

use proc_macro2::{TokenStream, TokenTree};
use std::env;
use std::fs;
use syn;

use anyhow::{Context, Result};
use probe_rs::{flashing, Probe};
use structopt::StructOpt;

mod building;

#[derive(Debug, StructOpt)]
#[structopt(name = "cargo-rtic-trace", about = "TODO")]
struct Opt {
    /// Binary to flash and trace.
    #[structopt(long = "bin")]
    bin: String,

    // TODO handle --example (or forward unknown arguments to rustc)
    /// Serial device over which trace stream is expected.
    #[structopt(long = "serial")]
    serial: String,

    /// Don't attempt to resolve hardware or software tasks.
    #[structopt(long = "dont-resolve")]
    dont_resolve: bool,
}

fn main() -> Result<()> {
    let opt = Opt::from_iter(env::args().skip(1)); // first argument is the subcommand name

    // attach target early to fail fast on any I/O issues
    let probe = Probe::list_all()[0]
        .open()
        .context("Unable to open probe")?;
    let mut session = probe
        .attach("stm32f401re")
        .context("Unable to attach to probe")?;

    // build wanted binary
    let artifact = building::cargo_build(&opt.bin)?;
    println!("{:?}", artifact);

    // ensure serial port is properly configured
    let port = {
        let mut stty = std::process::Command::new("stty");
        stty.args(&["-F", opt.serial.as_str()]);
        stty.arg("406:0:18b2:8a30:3:1c:7f:15:4:2:64:0:11:13:1a:0:12:f:17:16:0:0:0:0:0:0:0:0:0:0:0:0:0:0:0:0");
        let mut child = stty.spawn()?;
        let _ = child.wait()?;

        serialport::new(opt.serial, 115_200)
            .open()
            .context("Failed to open serial port")?
    };

    // resolve the data we need from RTIC app decl.
    if !opt.dont_resolve {
        // parse the RTIC app from the source file
        let src = fs::read_to_string(artifact.target.src_path)
            .context("Failed to open RTIC app source file")?;
        let mut rtic_app = syn::parse_str::<TokenStream>(&src)
            .context("Failed to parse RTIC app source file")?
            .into_iter()
            .skip_while(|token| {
                // TODO improve this?
                if let TokenTree::Group(g) = token {
                    return g.stream().into_iter().nth(0).unwrap().to_string().as_str() != "app";
                }
                true
            });
        let args = {
            let mut args: Option<TokenStream> = None;
            if let TokenTree::Group(g) = rtic_app.next().unwrap() {
                // TODO improve this?
                if let TokenTree::Group(g) = g.stream().into_iter().nth(1).unwrap() {
                    args = Some(g.stream());
                }
            }
            args.unwrap()
        };
        let app = rtic_app.collect::<TokenStream>();

        println!("Hardware tasks:");
        for (int, (fun, ex_ident)) in rtic_trace::parsing::hardware_tasks(app.clone(), args)? {
            println!("{} binds {} ({})", fun[1], ex_ident, int);
        }

        println!("Software tasks:");
        for (k, v) in rtic_trace::parsing::software_tasks(app)? {
            println!("({}, {:?})", k, v);
        }
    }

    // flash binary with probe.rs
    println!("Flashing {}...", opt.bin);
    // TODO use a progress bar alike cargo-flash
    flashing::download_file(
        &mut session,
        &artifact.executable.unwrap(),
        flashing::Format::Elf,
    )
    .context("Unable to flash target firmware")?;
    println!("Flashed.");

    // reset the target and execute flashed firmware
    println!("Resetting target...");
    let mut core = session.core(0)?;
    core.reset().context("Unable to reset target")?;
    println!("Reset.");

    // TODO collect trace until some stop condition

    // TODO save trace somewhere for offline analysis.

    // println!("Hello, world!");

    // Note: the first argument is the name of this binary
    // (`cargo-rtic-trace`), which we do not care about.

    Ok(())
}
