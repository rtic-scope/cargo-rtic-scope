use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{anyhow, bail, Context, Result};
use probe_rs::flashing;
use probe_rs_cli_util::flash::{list_connected_probes, print_families, CargoOptions, FlashOptions};
use structopt::StructOpt;

mod build;
mod recovery;
mod sinks;
mod sources;

#[derive(Debug, StructOpt)]
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
    /// Optional serial device over which trace stream is expected,
    /// instead of a CMSIS-DAP device.
    #[structopt(long = "serial")]
    serial: Option<String>,

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

    /// Speed in Hz of the TPIU trace clock. Used to calculate
    /// timestamps of received timestamps. If not set, a DataTraceValue
    /// packet containing the frequency is expected in the trace stream.
    #[structopt(long = "tpiu-freq")]
    trace_clk_freq: Option<u32>,

    #[structopt(flatten)]
    flash_options: FlashOptions,
}

/// Replay a previously recorded trace stream for post-mortem analysis.
#[derive(StructOpt, Debug)]
struct ReplayOpts {
    #[structopt(name = "list", long = "list", short = "l")]
    list: bool,

    #[structopt(required_unless("list"))]
    index: Option<usize>,

    /// Directory where previously recorded trace streams. By default,
    /// the build cache of <bin> is used (usually ./target/).
    #[structopt(name = "trace-dir", long = "trace-dir", parse(from_os_str))]
    trace_dir: Option<PathBuf>,

    #[structopt(flatten)]
    cargo_options: CargoOptions,
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

    // Setup SIGINT handler
    let halt = Arc::new(AtomicBool::new(false));
    let h = halt.clone();
    ctrlc::set_handler(move || {
        h.store(true, Ordering::SeqCst);
    })
    .context("Failed to install SIGINT handler")?;

    // Configure source and sinks. Recover the information we need to
    // map IRQ numbers to RTIC tasks.
    let (mut source, mut sinks, metadata) = match opts.cmd {
        Command::Trace(ref opts) => match trace(opts)? {
            Some(tup) => tup,
            None => return Ok(()), // NOTE --list-{chips,probes} passed
        },
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

    if let sources::BufferStatus::Unknown = source.avail_buffer() {
        eprintln!("Buffer size of source could not be found. Buffer may overflow and corrupt trace stream without warning.");
    }

    // All preparatory I/O and information recovery done. Forward all
    // trace packets to all sinks.
    //
    // Keep tabs on which sinks have broken during drain, if any.
    let mut sinks: Vec<(Box<dyn sinks::Sink>, bool)> =
        sinks.drain(..).map(|s| (s, false)).collect();
    let mut drain_status = Ok(());
    let mut buffer_warning = false;
    while let Some(packets) = source.next() {
        if halt.load(Ordering::SeqCst) {
            break;
        }

        // Eventually warn about the source input buffer overflowing,
        // but only once.
        if !buffer_warning {
            if let sources::BufferStatus::AvailWarn(avail, buf_sz) = source.avail_buffer() {
                eprintln!(
                "Source buffer is almost full ({}/{} bytes free) and it not read quickly enough",
                avail, buf_sz
                );
                buffer_warning = true;
            }
        }

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

        // remove broken sinks, if any
        //
        // TODO replace weth Vec::drain_filter when stable.
        sinks.retain(|(_, is_broken)| !is_broken);
        if sinks.is_empty() {
            drain_status = Err(anyhow!("All sinks broken. Cannot continue."));
            break;
        }
    }

    // close frontend sockets
    drop(sinks);

    // Wait for frontend to proccess all packets and echo its stderr
    {
        let status = child.wait();
        for err in BufReader::new(stderr).lines() {
            eprintln!(
                "{}: {}",
                opts.frontend,
                err.context("Failed to read frontend stderr")?
            );
        }
        if let Err(err) = status {
            eprintln!("Frontend {} exited with error: {}", opts.frontend, err);
        }
    }

    drain_status
}

fn build_target_binary(opts: &CargoOptions) -> Result<(build::CargoWrapper, build::Artifact)> {
    // Ensure we have a working cargo
    let mut cargo = build::CargoWrapper::new(vec![]).context("Failed to setup cargo")?;

    // Build the wanted binary
    let artifact = cargo.build(
        &env::current_dir()?,
        match opts {
            CargoOptions {
                bin: Some(bin),
                example: None,
                ..
            } => format!("--bin {}", bin),
            CargoOptions {
                bin: None,
                example: Some(example),
                ..
            } => format!("--example {}", example),
            CargoOptions {
                bin: None,
                example: None,
                ..
            } => bail!("Missing expected --bin or --example option"),
            CargoOptions {
                bin: Some(_),
                example: Some(_),
                ..
            } => bail!("Ambiguity error: please specify only --bin or --example"),
        },
        "bin",
    )?;

    // Internally resolve the build cache used when building the
    // artifact. It will henceforth be reused by all subsequent build
    // operations and as the default directory for saving recorded
    // traces.
    cargo.resolve_target_dir(&artifact)?;

    Ok((cargo, artifact))
}

type TraceTuple = (
    Box<dyn sources::Source>,
    Vec<Box<dyn sinks::Sink>>,
    recovery::Metadata,
);

fn trace(opts: &TraceOpts) -> Result<Option<TraceTuple>> {
    if opts.flash_options.list_probes {
        list_connected_probes(std::io::stdout()).context("Failed to list connected probes")?;
        return Ok(None);
    }

    if opts.flash_options.list_chips {
        print_families(std::io::stdout()).context("Failed to list chip families")?;
        return Ok(None);
    }

    let (cargo, artifact) = build_target_binary(&opts.flash_options.cargo_options)
        .context("Failed to build target binary")?;

    let mut session = opts
        .flash_options
        .target_session()
        .context("Failed to attach to target session")?;

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
    let elf = artifact.executable.unwrap();
    eprintln!("Flashing {}...", elf.display());
    flashing::download_file(&mut session, &elf, flashing::Format::Elf)
        .context("Failed to flash target firmware")?;
    eprintln!("Flashed.");

    let mut trace_source: Box<dyn sources::Source> = if let Some(dev) = &opts.serial {
        Box::new(sources::TTYSource::new(
            sources::tty::configure(&dev)
                .with_context(|| format!("Failed to configure {}", dev))?,
            session,
        ))
    } else {
        Box::new(sources::DAPSource::new(session)?)
    };

    // Sample the timestamp of target reset, wait for trace clock
    // frequency payload, flush metadata to file.
    let metadata = trace_sink
        .init(maps, || {
            // Reset the target to  execute flashed binary
            eprintln!("Resetting target...");
            // let mut core = session.core(0)?;
            // core.reset().context("Unable to reset target")?;
            trace_source.reset_target()?;
            eprintln!("Reset.");

            let freq = if let Some(freq) = opts.trace_clk_freq {
                freq
            } else {
                // Wait for a non-zero trace clock frequency payload.
                //
                // NOTE A side-effect here is that we effectively disregard all
                // packets that are emitted during target initialization. In
                // local tests these packets have been but noise, so this is
                // okay for the moment.
                sources::wait_for_trace_clk_freq(&mut trace_source)
                    .context("Failed to read trace clock frequency from source")?
            };

            Ok(freq)
        })
        .context("Failed to initialize metadata")?;

    Ok(Some((trace_source, vec![Box::new(trace_sink)], metadata)))
}

fn replay(opts: &ReplayOpts) -> Result<Option<TraceTuple>> {
    let mut traces = sinks::file::find_trace_files({
        if let Some(ref dir) = opts.trace_dir {
            dir.to_path_buf()
        } else {
            let (cargo, _artifact) = build_target_binary(&opts.cargo_options)?;
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
