// We want to be able to run
//
//     $ cargo rtic-trace --bin blinky --serial /dev/ttyUSB3
//

use std::env;
use std::io::Read;

use anyhow::{bail, Context, Result};
use itm_decode::{self, DecoderState};
use probe_rs::{flashing, Probe};
use structopt::StructOpt;

mod building;
mod parsing;
mod serial;

#[derive(Debug, StructOpt)]
#[structopt(name = "cargo-rtic-trace", about = "TODO")]
struct Opt {
    /// Binary to flash and trace.
    #[structopt(long = "bin")]
    bin: String,

    // TODO handle --example
    /// Serial device over which trace stream is expected.
    #[structopt(long = "serial")]
    serial: String,

    /// Flags forwarded to cargo. For example: -- --target-dir...
    cargo_flags: Vec<String>,
}

fn main() -> Result<()> {
    let opt = Opt::from_iter(
        // NOTE(skip): first argument is the subcommand name
        env::args().skip(1),
    );

    // Attach to target and prepare serial. We want to fail fast on any
    // I/O issues.
    //
    // TODO allow user to specify probe and target
    let probes = Probe::list_all();
    if probes.is_empty() {
        bail!("No supported target probes found");
    }
    let probe = probes[0].open().context("Unable to open first probe")?;
    let mut session = probe
        .attach("stm32f401re")
        .context("Unable to attach to stm32f401re")?;
    let trace_tty = serial::configure(&opt.serial)
        .with_context(|| format!("Failed to configure {}", opt.serial))?;

    // Ensure we have a working cargo
    let mut cargo = building::CargoWrapper::new(opt.cargo_flags)
        .context("Unable to find a working cargo executable")?;

    // Build the wanted binary
    let artifact = cargo.build(
        &env::current_dir()?,
        format!("--bin {}", opt.bin),
        "bin",
    )?;

    // Internally resolve the build cache used when building the
    // artifact. It will henceforth be reused by all subsequent build
    // operations and as the default directory for saving recorded
    // traces.
    cargo.resolve_target_dir(&artifact)?;

    // Map IRQ numbers to their respective tasks
    let ((excps, ints), sw_tasks) = parsing::TaskResolver::new(&artifact, &cargo)
        .context("Failed to parse RTIC application source file")?
        .resolve()
        .context("Failed to resolve tasks")?;

    println!("int: {:?}, ext: {:?}", ints, excps);
    println!("Software tasks:");
    for (k, v) in sw_tasks {
        println!("({}, {:?})", k, v);
    }

    // Flash binary to target
    //
    // TODO use a progress bar alike cargo-flash
    //
    // TODO skip flash if correct bin already flashed. We check that by
    // downloding the binary and comparing hashes (unless there is HW
    // that does this for us)
    println!("Flashing {}...", opt.bin);
    flashing::download_file(
        &mut session,
        &artifact.executable.unwrap(),
        flashing::Format::Elf,
    )
    .context("Unable to flash target firmware")?;
    println!("Flashed.");

    // TODO sample a timestamp here

    // Reset the target and execute flashed binary
    println!("Resetting target...");
    let mut core = session.core(0)?;
    core.reset().context("Unable to reset target")?;
    println!("Reset.");

    // TODO open a file under blinky/target/rtic-traces here, make so
    // that we can toss data to it async we wan to write

    // TODO find target/

    let mut decoder = itm_decode::Decoder::new();
    for byte in trace_tty.bytes() {
        decoder.push([byte.context("Failed to read byte from trace tty")?].to_vec());
        loop {
            match decoder.pull() {
                Ok(None) => break,
                Ok(Some(packet)) => println!("{:?}", packet),
                Err(e) => {
                    println!("Error: {:?}", e);
                    decoder.state = DecoderState::Header;
                }
            }
        }
    }

    // TODO save trace somewhere for offline analysis.

    Ok(())
}
