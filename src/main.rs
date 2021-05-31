// We want to be able to run
//
//     $ cargo rtic-trace --bin blinky --serial /dev/ttyUSB3
//

use proc_macro2::{TokenStream, TokenTree};
use std::env;
use std::fs;
use syn;

use anyhow::{Context, Result};
use structopt::StructOpt;

mod building;

#[derive(Debug, StructOpt)]
#[structopt(name = "cargo-rtic-trace", about = "TODO")]
struct Opt {
    /// Binary to flash and trace.
    #[structopt(long = "bin")]
    bin: String,

    // TODO handle example (or forward unknown arguments to rustc)
    /// Serial device over which trace stream is expected.
    #[structopt(long = "serial")]
    serial: String,
}

fn main() -> Result<()> {
    let opt = Opt::from_iter(env::args().skip(1));

    // build wanted binary
    let artifact = building::cargo_build(&opt.bin)?;
    println!("{:?}", artifact);

    // TODO ensure --serial is properly configured

    // resolve the data we need from RTIC app decl.
    {
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

    // TODO flash binary with probe.rs

    // TODO properly run the flashed binary

    // TODO collect trace until some stop condition

    // TODO save trace somewhere for offline analysis.

    // println!("Hello, world!");

    // Note: the first argument is the name of this binary
    // (`cargo-rtic-trace`), which we do not care about.

    Ok(())
}
