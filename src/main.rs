use std::env;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use probe_rs::{flashing, Probe};
use rtic_scope_api::{self as api, Frontend};
use structopt::StructOpt;

mod build;
mod parse;
mod serial;
mod trace;

#[derive(Debug, StructOpt)]
struct Opts {
    #[structopt(subcommand)]
    cmd: Command,
}

/// Execute and trace a chosen application on a target device and record
/// the trace stream to file.
#[derive(StructOpt, Debug)]
struct TraceOpts {
    /// Binary to flash and trace.
    #[structopt(long = "bin")]
    bin: String,

    // TODO handle --example
    /// Serial device over which trace stream is expected.
    #[structopt(long = "serial")]
    serial: String,

    /// Output directory for recorded trace streams. By default, the
    /// build chache of <bin> is used (usually ./target/).
    #[structopt(long = "trace-dir", parse(from_os_str))]
    trace_dir: Option<PathBuf>,

    // TODO utilize
    /// Arbitrary comment that describes the trace. Currently not in
    /// use.
    #[structopt(long = "comment", short = "c")]
    trace_comment: Option<String>,

    /// Remove all previous traces from <trace-dir>.
    #[structopt(long = "clear-traces")]
    remove_prev_traces: bool,
}

/// Replay a previously recorded trace stream for post-mortem analysis.
#[derive(StructOpt, Debug)]
struct ReplayOpts {
    #[structopt(name = "list", long = "list", short = "l")]
    list: bool,

    #[structopt(required_unless("list"))]
    index: Option<usize>,

    /// Target binary from which to resolve the build cache and thus
    /// previously recorded trace streams.
    #[structopt(long = "bin", required_unless("trace-dir"))]
    bin: Option<String>,

    /// Directory where previously recorded trace streams. By default,
    /// the build cache of <bin> is used (usually ./target/).
    #[structopt(name = "trace-dir", long = "trace-dir", parse(from_os_str))]
    trace_dir: Option<PathBuf>,
}

#[derive(StructOpt, Debug)]
enum Command {
    Trace(TraceOpts),
    Replay(ReplayOpts),
}

fn main() -> Result<()> {
    let opts = Opts::from_iter(
        // NOTE(skip): first argument is the subcommand name
        env::args().skip(1),
    );

    let (tx, rx) = std::sync::mpsc::channel();
    let frontend = rtic_scope_frontend_dummy::Dummy::spawn(rx)?;

    match opts.cmd {
        Command::Trace(opts) => trace(opts, tx)?,
        Command::Replay(opts) => replay(opts, tx)?,
    };

    frontend
        .join()
        .expect("Failed to join frontend thread")
        .context("Frontend reported error")?;

    Ok(())
}

// TODO Rethink this
fn build_target_binary(
    bin: &String,
    cargo_flags: Vec<String>,
) -> Result<(build::CargoWrapper, build::Artifact)> {
    // Ensure we have a working cargo
    let mut cargo = build::CargoWrapper::new(cargo_flags).context("Failed to setup cargo")?;

    // Build the wanted binary
    let artifact = cargo.build(&env::current_dir()?, format!("--bin {}", bin), "bin")?;

    // Internally resolve the build cache used when building the
    // artifact. It will henceforth be reused by all subsequent build
    // operations and as the default directory for saving recorded
    // traces.
    cargo.resolve_target_dir(&artifact)?;

    Ok((cargo, artifact))
}

fn trace(opts: TraceOpts, tx: std::sync::mpsc::Sender<api::EventChunk>) -> Result<()> {
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

    let trace_tty = serial::configure(&opts.serial)
        .with_context(|| format!("Failed to configure {}", opts.serial))?;

    let (cargo, artifact) = build_target_binary(&opts.bin, vec![])?;

    // TODO make this into Sink::generate().remove_old(), etc.
    let mut trace_sink = trace::Sink::generate_trace_file(
        &artifact,
        &opts
            .trace_dir
            .unwrap_or(cargo.target_dir().unwrap().join("rtic-traces")),
        opts.remove_prev_traces,
    )
    .context("Failed to generate trace sink file")?;

    // Map IRQ numbers and DWT matches to their respective RTIC tasks
    let maps = parse::TaskResolver::new(&artifact, &cargo)
        .context("Failed to parse RTIC application source file")?
        .resolve()
        .context("Failed to resolve tasks")?;
    println!("{}", &maps);

    // Flash binary to target
    //
    // TODO use a progress bar alike cargo-flash
    //
    // TODO skip flash if correct bin already flashed. We check that by
    // downloding the binary and comparing hashes (unless there is HW
    // that does this for us)
    println!("Flashing {}...", opts.bin);
    flashing::download_file(
        &mut session,
        &artifact.executable.unwrap(),
        flashing::Format::Elf,
    )
    .context("Failed to flash target firmware")?;
    println!("Flashed.");

    // Sample the timestamp of target reset, flush metadata to file.
    trace_sink.init(&maps, || {
        // Reset the target to  execute flashed binary
        println!("Resetting target...");
        let mut core = session.core(0)?;
        core.reset().context("Unable to reset target")?;
        println!("Reset.");

        Ok(())
    })?;

    println!("Tracing...");
    for byte in trace_tty.bytes() {
        trace_sink.push(
            &maps,
            &tx,
            byte.context("Failed to read byte from trace tty")?,
        )?;
    }

    Ok(())
}

fn replay(opts: ReplayOpts, tx: std::sync::mpsc::Sender<api::EventChunk>) -> Result<()> {
    let mut traces = trace::find_trace_files(&{
        if let Some(dir) = opts.trace_dir {
            dir
        } else {
            let (cargo, _artifact) = build_target_binary(&opts.bin.unwrap(), vec![])?;
            cargo.target_dir().unwrap().join("rtic-traces")
        }
    })?;

    if opts.list {
        for (i, trace) in traces.enumerate() {
            println!("{}\t{}", i, trace.display());
        }
    } else if let Some(idx) = opts.index {
        let trace = traces
            .nth(idx)
            .with_context(|| format!("No trace with index {}", idx))?;
        println!("Replaying {}", trace.display());

        // open trace file and print packets (for now)
        let mut src = trace::Source::open(trace).context("Failed to open trace file")?;
        let maps = src.copy_maps();
        for p in src.iter() {
            let p = p?;
            match maps
                .resolve_tasks(p.clone())
                .with_context(|| format!("Failed to resolve tasks for packets {:?}", p))
            {
                Ok(packets) => {
                    tx.send(packets)
                        .context("Failed to send EventChunk to frontend")?;
                }
                Err(e) => eprintln!("{}, ignoring...", e),
            }
        }
    }

    Ok(())
}
