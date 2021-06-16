// We want to be able to run
//
//     $ cargo rtic-trace --bin blinky --serial /dev/ttyUSB3
//

use std::env;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use probe_rs::{flashing, Probe};
use structopt::StructOpt;

mod building;
mod parsing;
mod serial;
mod tracing;

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

    /// Output directory for recorded trace streams. By default, the
    /// build chache of <bin> is used (usually ./target/).
    #[structopt(long = "output", short = "o", parse(from_os_str))]
    trace_dir: Option<PathBuf>,

    // TODO utilize
    /// Arbitrary comment that describes the trace. Currently not in
    /// use.
    #[structopt(long = "comment", short = "c")]
    trace_comment: Option<String>,

    /// Remove all previous traces from <trace-dir>.
    #[structopt(long = "clear-traces")]
    remove_prev_traces: bool,

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
        .context("Failed to attach to stm32f401re")?;
    let trace_tty = serial::configure(&opt.serial)
        .with_context(|| format!("Failed to configure {}", opt.serial))?;

    // Ensure we have a working cargo
    let mut cargo =
        building::CargoWrapper::new(opt.cargo_flags).context("Failed to setup cargo")?;

    // Build the wanted binary
    let artifact = cargo.build(&env::current_dir()?, format!("--bin {}", opt.bin), "bin")?;

    // Internally resolve the build cache used when building the
    // artifact. It will henceforth be reused by all subsequent build
    // operations and as the default directory for saving recorded
    // traces.
    cargo.resolve_target_dir(&artifact)?;

    // TODO make this into Sink::generate().remove_old(), etc.
    let mut trace_sink = tracing::Sink::generate(
        &artifact,
        &opt.trace_dir
            .unwrap_or(cargo.target_dir().unwrap().join("rtic-traces")),
        opt.remove_prev_traces,
    )
    .context("Failed to generate trace sink file")?;

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
    .context("Failed to flash target firmware")?;
    println!("Flashed.");

    // XXX must be done before resetting target
    trace_sink.sample_reset_timestamp()?;

    // XXX Time to execute the below block should be predictable. We
    // want to do as little as possible between timestamping and
    // processing the trace bytes.
    {
        // Reset the target and execute flashed binary
        println!("Resetting target...");
        let mut core = session.core(0)?;
        core.reset().context("Unable to reset target")?;
        println!("Reset.");
        println!("Tracing...");
    }

    for byte in trace_tty.bytes() {
        trace_sink.push(byte.context("Failed to read byte from trace tty")?)?;
    }

    Ok(())
}
