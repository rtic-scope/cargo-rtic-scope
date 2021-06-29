use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process;

use anyhow::{bail, Context, Result};
use probe_rs::{flashing, Probe};
use structopt::StructOpt;

mod build;
mod recovery;
mod serial;
mod sinks;
mod sources;

#[derive(Debug, StructOpt)]
#[structopt(version = env!("VERGEN_GIT_SEMVER"))]
struct Opts {
    /// The frontend to forward recorded/replayed trace to.
    #[structopt(long = "frontend", short = "-F", default_value = "dummy")]
    frontend: String,

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

    /// Frequency of the trace clock.
    #[structopt(long = "freq")]
    trace_clk_freq: usize,
}

/// Replay a previously recorded trace stream for post-mortem analysis.
#[derive(StructOpt, Debug, Clone)]
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

    // Configure source and sinks. Recover the information we need to
    // map IRQ numbers to RTIC tasks.
    let (source, mut sinks, metadata) = match opts.cmd {
        Command::Trace(ref opts) => trace(opts).context("Failed to capture trace")?.unwrap(), // NOTE(unwrap): trace always returns Some
        Command::Replay(ref opts) => {
            match replay(opts).with_context(|| {
                format!("Failed to {}", {
                    if opts.list {
                        "index traces"
                    } else {
                        "replay trace"
                    }
                })
            })? {
                Some(tup) => tup,
                None => return Ok(()), // NOTE --list was passed
            }
        }
    };

    eprintln!("{}", &metadata);

    // Spawn frontend child and get path to socket. Create and push sink.
    let mut child = process::Command::new(format!("rtic-scope-frontend-{}", opts.frontend))
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .context("Failed to spawn frontend child process")?;
    {
        let socket_path = {
            std::io::BufReader::new(
                child
                    .stdout
                    .take()
                    .context("Failed to pipe frontend stdout")?,
            )
            .lines()
            .next()
            .context("next() failed")?
        }
        .context("Failed to read socket path from frontend child process")?;
        let socket = std::os::unix::net::UnixStream::connect(&socket_path)
            .context("Failed to connect to frontend socket")?;
        sinks.push(Box::new(sinks::FrontendSink::new(socket, metadata)));
    }
    let stderr = child
        .stderr
        .take()
        .context("Failed to pipe frontend stderr")?;

    // All preparatory I/O and information recovery done. Forward all
    // trace packets to all sinks.
    //
    // Keep tabs on which sinks have broken during drain, if any.
    let mut sinks: Vec<(Box<dyn sinks::Sink>, bool)> =
        sinks.drain(..).map(|s| (s, false)).collect();
    for packets in source.into_iter() {
        let packets = packets.context("Failed to read trace packets from source")?;

        for (sink, is_broken) in sinks.iter_mut() {
            if let Err(e) = sink.drain(packets.clone()) {
                eprintln!(
                    "Failed to drain trace packets to {}: {:?}",
                    sink.describe(),
                    e
                );
                *is_broken = true;
            }
        }

        // remove all broken sinks
        //
        // TODO replace weth Vec::drain_filter when stable.
        sinks.retain(|(_, is_broken)| !is_broken);
        if sinks.is_empty() {
            break;
        }
    }

    // close socket to frontend
    let sinks_broken = sinks.is_empty();
    drop(sinks);

    // TODO make sure the below is executed on SIGINT

    // Wait for frontend to proccess all packets and echo its stderr
    {
        let status = child.wait();
        for err in BufReader::new(stderr).lines() {
            eprintln!("{}: {}", opts.frontend, err?);
        }
        status.with_context(|| format!("Frontend {} exited with error", opts.frontend))?;
    }

    if sinks_broken {
        bail!("All sinks broken. Cannot continue.");
    }

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

type TraceTuple = (
    Box<dyn Iterator<Item = Result<itm_decode::TimestampedTracePackets>>>,
    Vec<Box<dyn sinks::Sink>>,
    recovery::Metadata,
);

fn trace(opts: &TraceOpts) -> Result<Option<TraceTuple>> {
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

    let trace_tty = sources::TtySource::new(
        serial::configure(&opts.serial)
            .with_context(|| format!("Failed to configure {}", opts.serial))?,
    );

    let (cargo, artifact) = build_target_binary(&opts.bin, vec![])?;

    // TODO make this into Sink::generate().remove_old(), etc.
    let mut trace_sink = sinks::FileSink::generate_trace_file(
        &artifact,
        opts.trace_dir
            .as_ref()
            .unwrap_or(&cargo.target_dir().unwrap().join("rtic-traces")),
        opts.remove_prev_traces,
    )
    .context("Failed to generate trace sink file")?;

    // Map IRQ numbers and DWT matches to their respective RTIC tasks
    let maps = recovery::TaskResolver::new(&artifact, &cargo)
        .context("Failed to parse RTIC application source file")?
        .resolve()
        .context("Failed to resolve tasks")?;

    // Flash binary to target
    //
    // TODO use a progress bar alike cargo-flash
    //
    // TODO skip flash if correct bin already flashed. We check that by
    // downloding the binary and comparing hashes (unless there is HW
    // that does this for us)
    eprintln!("Flashing {}...", opts.bin);
    flashing::download_file(
        &mut session,
        &artifact.executable.unwrap(),
        flashing::Format::Elf,
    )
    .context("Failed to flash target firmware")?;
    eprintln!("Flashed.");

    // Sample the timestamp of target reset, flush metadata to file.
    let metadata = trace_sink.init(maps, opts.trace_clk_freq, || {
        // Reset the target to  execute flashed binary
        eprintln!("Resetting target...");
        let mut core = session.core(0)?;
        core.reset().context("Unable to reset target")?;
        eprintln!("Reset.");

        Ok(())
    })?;

    Ok(Some((
        Box::new(trace_tty),
        vec![Box::new(trace_sink)],
        metadata,
    )))
}

fn replay(opts: &ReplayOpts) -> Result<Option<TraceTuple>> {
    let mut traces = sinks::find_trace_files({
        if let Some(ref dir) = opts.trace_dir {
            dir.to_path_buf()
        } else {
            let (cargo, _artifact) = build_target_binary(opts.bin.as_ref().unwrap(), vec![])?;
            cargo.target_dir().unwrap().join("rtic-traces")
        }
    })?;

    if opts.list {
        for (i, trace) in traces.enumerate() {
            println!("{}\t{}", i, trace.display());
        }

        return Ok(None);
    } else if let Some(idx) = opts.index {
        let trace = traces
            .nth(idx)
            .with_context(|| format!("No trace with index {}", idx))?;
        eprintln!("Replaying {}", trace.display());

        // open trace file and print packets (for now)
        let src = sources::FileSource::new(fs::OpenOptions::new().read(true).open(&trace)?)?;
        let metadata = src.metadata();

        return Ok(Some((Box::new(src), vec![], metadata)));
    }

    unreachable!();
}
