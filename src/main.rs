use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{anyhow, Context};
use cargo_metadata::Artifact;
use probe_rs_cli_util::{
    common_options::{CargoOptions, FlashOptions},
    flash,
};
use rtic_scope_api as api;
use structopt::StructOpt;
use thiserror::Error;

mod build;
mod diag;
mod log;
mod pacp;
mod recovery;
mod sinks;
mod sources;

use build::CargoWrapper;

pub type TraceData = itm_decode::TimestampedTracePackets;

#[derive(Debug, StructOpt)]
struct Opts {
    /// The frontend to forward recorded/replayed trace to.
    #[structopt(long = "frontend", short = "-F", default_value = "dummy")]
    frontends: Vec<String>,

    #[structopt(subcommand)]
    cmd: Command,
}

/// Execute and trace a chosen application on a target device and record
/// the trace stream to file.
#[derive(StructOpt, Debug)]
struct TraceOptions {
    /// Optional serial device over which trace stream is expected,
    /// instead of a CMSIS-DAP device.
    #[structopt(name = "serial", long = "serial")]
    serial: Option<String>,

    /// Output directory for recorded trace streams. By default, the
    /// build chache of <bin> is used (usually ./target/).
    #[structopt(long = "trace-dir", parse(from_os_str))]
    trace_dir: Option<PathBuf>,

    /// Arbitrary comment that describes the trace.
    #[structopt(long = "comment", short = "c")]
    comment: Option<String>,

    /// Remove all previous traces from <trace-dir>.
    #[structopt(long = "clear-traces")]
    remove_prev_traces: bool,

    #[structopt(flatten)]
    pac: PACOptions,

    #[structopt(flatten)]
    tpiu: TPIUOptions,

    #[structopt(flatten)]
    flash_options: FlashOptions,
}

#[derive(StructOpt, Debug)]
pub struct PACOptions {
    /// Name of the PAC used in traced application.
    #[structopt(long = "pac", name = "pac")]
    name: Option<String>,

    /// Version of the PAC used in the traced application.
    #[structopt(long = "pac-version", name = "pac-version")]
    version: Option<String>,

    /// Features of the PAC used in traced application.
    #[structopt(long = "pac-features", name = "pac-features")]
    features: Option<Vec<String>>,

    /// Path to PAC Interrupt enum.
    #[structopt(long = "pac-interrupt-path")]
    interrupt_path: Option<String>,
}

#[derive(StructOpt, Debug)]
pub struct TPIUOptions {
    /// Speed in Hz of the TPIU trace clock. Used to calculate
    /// timestamps of received timestamps.
    #[structopt(long = "tpiu-freq")]
    clk_freq: u32,

    // Baud rate of the communication from the target TPIU.
    #[structopt(long = "tpiu-baud", default_value = "2000000")]
    baud_rate: u32,
}

/// Replay a previously recorded trace stream for post-mortem analysis.
#[derive(StructOpt, Debug)]
struct ReplayOptions {
    #[structopt(name = "list", long = "list", short = "l")]
    list: bool,

    #[structopt(required_unless_one(&["list", "raw-file"]))]
    index: Option<usize>,

    #[structopt(flatten)]
    raw_options: RawFileOptions,

    /// Directory where previously recorded trace streams. By default,
    /// the build cache of <bin> is used (usually ./target/).
    #[structopt(name = "trace-dir", long = "trace-dir", parse(from_os_str))]
    trace_dir: Option<PathBuf>,

    #[structopt(flatten)]
    cargo_options: CargoOptions,
}

#[derive(StructOpt, Debug)]
struct RawFileOptions {
    /// Path to the file containing raw trace data that should be
    /// replayed.
    #[structopt(name = "raw-file", long = "raw-file", requires("virtual-freq"))]
    file: Option<PathBuf>,

    #[structopt(name = "virtual-freq", long = "tpiu-freq", hidden = true)]
    freq: u32,
    #[structopt(long = "comment", short = "c", hidden = true)]
    comment: Option<String>,
    #[structopt(flatten)]
    pac: PACOptions,
}

#[derive(StructOpt, Debug)]
enum Command {
    Trace(TraceOptions),
    Replay(ReplayOptions),
}

#[derive(Debug, Error)]
pub enum RTICScopeError {
    // adhoc errors
    // TODO remove?
    #[error("Probe setup and/or initialization failed: {0}")]
    CommonProbeOperationError(#[from] probe_rs_cli_util::common_options::OperationError),
    #[error("I/O operation failed: {0}")]
    IOError(#[from] std::io::Error),

    // transparent errors
    #[error(transparent)]
    PACError(#[from] pacp::PACMetadataError),
    #[error(transparent)]
    MetadataError(#[from] recovery::RecoveryError),
    #[error(transparent)]
    CargoError(#[from] build::CargoError),
    #[error(transparent)]
    SourceError(#[from] sources::SourceError),
    #[error(transparent)]
    SinkError(#[from] sinks::SinkError),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl diag::DiagnosableError for RTICScopeError {
    fn diagnose(&self) -> Vec<String> {
        match self {
            RTICScopeError::PACError(_) => vec![
                "PAC properties can be set via --pac, --pac-features, --pac-interrupt-path".to_string(),
                "[package.metadata.rtic-scope] takes precedence over [workspace.metadata.rtic-scope]".to_string(),
            ],
            _ => vec![],
        }
    }
}

impl RTICScopeError {
    pub fn render(&self) {
        log::err(format!("{:?}", self));

        // print eventual hints
        use crate::diag::DiagnosableError;
        type DE = dyn DiagnosableError;
        for hint in self.diagnose().iter().chain(
            match self {
                Self::PACError(e) => Some(e as &DE),
                Self::MetadataError(e) => Some(e as &DE),
                Self::CargoError(e) => Some(e as &DE),
                Self::SourceError(e) => Some(e as &DE),
                Self::SinkError(e) => Some(e as &DE),
                _ => None,
            }
            .and_then(|e| Some(e.diagnose()))
            .unwrap_or(vec![])
            .iter(),
        ) {
            log::hint(hint.to_owned());
        }
    }
}

fn main() {
    if let Err(e) = main_try() {
        e.render();
        std::process::exit(1); // TODO make retval depend on error type?
    }
}

fn main_try() -> Result<(), RTICScopeError> {
    // Handle CLI options
    let mut args: Vec<_> = std::env::args().collect();
    // When called by cargo, first argument will be "rtic-scope".
    if args.get(1) == Some(&"rtic-scope".to_string()) {
        args.remove(1);
    }
    let matches = Opts::clap()
        .after_help(CargoOptions::help_message("cargo rtic-scope trace").as_str())
        .get_matches_from(&args);
    let opts = Opts::from_clap(&matches);

    // Should be quit early?
    if let Command::Trace(opts) = &opts.cmd {
        let fo = &opts.flash_options;
        fo.probe_options.maybe_load_chip_desc()?;
        if fo.early_exit(std::io::stdout())? {
            return Ok(());
        }
    }

    // Build RTIC application to be traced, and create a wrapper around
    // cargo, reusing the target directory of the application.
    let (cargo, artifact) = CargoWrapper::new(
        &env::current_dir()?,
        {
            match &opts.cmd {
                Command::Trace(opts) => &opts.flash_options.cargo_options,
                Command::Replay(opts) => &opts.cargo_options,
            }
        }
        .to_cargo_options(),
    )?;

    let prog = format!(
        "{} ({})",
        artifact.target.name,
        artifact.target.src_path.display()
    );
    log::status(
        "Recovering",
        format!("metadata for {} and preparing target...", prog,),
    );

    // Configure source and sinks. Recover the information we need to
    // map IRQ numbers to RTIC tasks.
    let (source, mut sinks, metadata) = match opts.cmd {
        Command::Trace(ref opts) => match trace(opts, &cargo, &artifact)? {
            Some(tup) => tup,
            None => return Ok(()),
        },
        Command::Replay(ref opts) => {
            match replay(opts, &cargo, &artifact).with_context(|| {
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

    log::status(
        "Recovered",
        format!(
            "{ntotal} task(s) from {prog}: {nhard} hard, {nsoft} soft. Target reset and flashed.",
            ntotal = metadata.hardware_tasks() + metadata.software_tasks(),
            prog = artifact.target.name,
            nhard = metadata.hardware_tasks(),
            nsoft = metadata.software_tasks()
        ),
    );

    // Spawn frontend children and get path to sockets. Create and push sinks.
    let mut children = vec![];
    for frontend in &opts.frontends {
        let executable = format!("rtic-scope-frontend-{}", frontend);
        let mut child = process::Command::new(&executable)
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn frontend child process {}", executable))?;
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
            sinks.push(Box::new(sinks::FrontendSink::new(socket)));
        }

        let stderr = child
            .stderr
            .take()
            .context("Failed to take frontend stderr")?;
        children.push((child, stderr));
    }

    if let sources::BufferStatus::Unknown = source.avail_buffer() {
        log::warn(format!(
            "buffer size of source {} could not be found; buffer may overflow and corrupt trace stream without further warning",
            source.describe())
        );
    }

    // All preparatory I/O and information recovery done. Forward all
    // trace packets to all sinks.
    let retstatus = run_loop(source, sinks, metadata.clone(), &opts, artifact.target.name);

    // Wait for frontends to proccess all packets and echo its' stderr
    for (i, (child, stderr)) in children.iter_mut().enumerate() {
        let status = child.wait();
        for err in BufReader::new(stderr).lines() {
            log::err(format!(
                "{}: {}",
                opts.frontends.get(i).unwrap(),
                err.context("Failed to read frontend stderr")?
            ));
        }
        if let Err(err) = status {
            log::err(format!(
                "frontend {} exited non-zero: {}",
                opts.frontends.get(i).unwrap(),
                err
            ));
        }
    }

    retstatus
}

fn run_loop(
    mut source: Box<dyn sources::Source>,
    mut sinks: Vec<Box<dyn sinks::Sink>>,
    metadata: recovery::Metadata,
    opts: &Opts,
    prog: String,
) -> Result<(), RTICScopeError> {
    // Setup SIGINT handler
    let halt = Arc::new(AtomicBool::new(false));
    let h = halt.clone();
    ctrlc::set_handler(move || {
        h.store(true, Ordering::SeqCst);
        println!("\n"); // separate sink errors from tracing status line, and don't erase it
    })
    .context("Failed to install SIGINT handler")?;

    // Keep tabs on which sinks have broken during drain, if any.
    let mut sinks: Vec<(Box<dyn sinks::Sink>, bool)> =
        sinks.drain(..).map(|s| (s, false)).collect();
    let sinks_at_start = sinks.len();

    let mut retstatus = Ok(());
    let mut buffer_warning = false;

    #[derive(Default)]
    struct Stats {
        // How many ITM packets we have received from the source...
        pub packets: usize,

        // ...of which, how many are malformed/nonmappable?
        pub malformed: usize,
        pub nonmappable: usize,
    }
    let mut stats = Stats::default();

    let instant = std::time::Instant::now();

    while let Some(data) = source.next() {
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

        // Try to read data from the source. Failure is fatal.
        let data = data.with_context(|| {
            format!(
                "Failed to read trace data from source {}",
                source.describe()
            )
        })?;

        // Try to recover RTIC information for the packets.
        let chunk = metadata.build_event_chunk(data.clone());

        // Report any unmappable/unknown events that occured, and record stats
        stats.packets = stats.packets + data.packets_consumed;
        for event in chunk.events.iter() {
            match event {
                api::EventType::Unmappable(ref packet, ref reason) => {
                    stats.nonmappable = stats.nonmappable + 1;
                    log::warn(format!(
                        "cannot map {:?} packet: {}",
                        packet, reason
                    ));
                }
                api::EventType::Unknown(ref packet) => {
                    stats.nonmappable = stats.nonmappable + 1;
                    log::warn(format!(
                        "cannot map {:?} packet",
                        packet
                    ));
                }
                api::EventType::Invalid(ref malformed) => {
                    stats.malformed = stats.malformed + 1;
                    log::err(format!("malformed packet: {}: {:?}", malformed, malformed));
                },
                api::EventType::Overflow => log::warn("Overflow detected! Packets may have been dropped and timestamps will be diverged until the next global timestamp".to_string()),
                _ => (),
            }
        }

        for (sink, is_broken) in sinks.iter_mut() {
            if let Err(e) = sink.drain(data.clone(), chunk.clone()) {
                log::err(format!(
                    "failed to drain trace packets to {}: {:?}",
                    sink.describe(),
                    e
                ));
                *is_broken = true;
            }
        }

        // remove broken sinks, if any
        //
        // TODO replace weth Vec::drain_filter when stable.
        sinks.retain(|(_, is_broken)| !is_broken);
        if sinks.is_empty() {
            retstatus = Err(anyhow!("All sinks broken. Cannot continue."));
            break;
        }

        let duration = instant.elapsed();
        log::cont_status(
            match opts.cmd {
                Command::Trace(_) => "Tracing",
                Command::Replay(_) => "Replaying",
            },
            format!(
                "{}: {} packets processed in {time} (~{packets_per_sec} packets/s; {} malformed, {} non-mappable); {sinks}...",
                prog,
                stats.packets,
                stats.malformed,
                stats.nonmappable,
                packets_per_sec = if duration.as_secs() != 0 {
                    stats.packets / duration.as_secs() as usize
                } else {
                    0
                },
                time = match duration.as_secs() {
                    duration if duration >= 60 * 60 => {
                        let secs = duration % 60;
                        let mins = (duration / 60) % 60;
                        let hours = duration / 60 / 60;

                        format!("{}h {}min {}s", hours, mins, secs)
                    }
                    duration if duration >= 60 => {
                        let secs = duration % 60;
                        let mins = (duration / 60) % 60;

                        format!("{}min {}s", mins, secs)
                    }
                    duration => {
                        let secs = duration % 60;

                        format!("{}s", secs)
                    }
                },
                sinks = format!("{}/{} sinks operational", sinks.len(), sinks_at_start),
            ),
        );
    }

    // close frontend sockets
    drop(sinks);

    retstatus.map_err(|e| e.into())
}

type TraceTuple = (
    Box<dyn sources::Source>,
    Vec<Box<dyn sinks::Sink>>,
    recovery::Metadata,
);

fn resolve_maps(
    cargo: &CargoWrapper,
    pac: &PACOptions,
    artifact: &Artifact,
) -> Result<recovery::TaskResolveMaps, RTICScopeError> {
    // Find crate name, features and path to interrupt enum from
    // manifest metadata, or override options.
    let pacp = pacp::PACProperties::new(cargo, pac)?;

    // Map IRQ numbers and DWT matches to their respective RTIC tasks
    let maps = recovery::TaskResolver::new(artifact, cargo, pacp)
        .context("Failed to parse RTIC application source file")?
        .resolve()
        .context("Failed to resolve tasks")?;

    Ok(maps)
}

fn trace(
    opts: &TraceOptions,
    cargo: &CargoWrapper,
    artifact: &Artifact,
) -> Result<Option<TraceTuple>, RTICScopeError> {
    let mut session = opts
        .flash_options
        .probe_options
        .simple_attach()
        .context("Failed to attach to target session")?;

    // TODO make this into Sink::generate().remove_old(), etc.
    let mut trace_sink = sinks::FileSink::generate_trace_file(
        &artifact,
        opts.trace_dir
            .as_ref()
            .unwrap_or(&cargo.target_dir().join("rtic-traces")),
        opts.remove_prev_traces,
    )
    .context("Failed to generate trace sink file")?;

    let maps = resolve_maps(cargo, &opts.pac, artifact)?;

    // Flash binary to target
    let elf = artifact.executable.as_ref().unwrap();
    let flashloader = opts
        .flash_options
        .probe_options
        .build_flashloader(&mut session, &elf)?;
    flash::run_flash_download(&mut session, &elf, &opts.flash_options, flashloader)?;

    let mut trace_source: Box<dyn sources::Source> = if let Some(dev) = &opts.serial {
        Box::new(sources::TTYSource::new(
            sources::tty::configure(&dev)
                .with_context(|| format!("Failed to configure {}", dev))?,
            session,
        ))
    } else {
        Box::new(sources::ProbeSource::new(session, &opts.tpiu)?)
    };

    // Sample the timestamp of target reset, wait for trace clock
    // frequency payload, flush metadata to file.
    let metadata = trace_sink
        .init(maps, opts.comment.clone(), || {
            // Reset the target to execute flashed binary
            trace_source.reset_target()?; // XXX halt-and-reset opt not handled

            Ok(opts.tpiu.clk_freq)
        })
        .context("Failed to initialize metadata")?;

    Ok(Some((trace_source, vec![Box::new(trace_sink)], metadata)))
}

fn replay(
    opts: &ReplayOptions,
    cargo: &CargoWrapper,
    artifact: &Artifact,
) -> Result<Option<TraceTuple>, RTICScopeError> {
    match opts {
        ReplayOptions {
            raw_options:
                RawFileOptions {
                    file: Some(file),
                    freq,
                    comment,
                    pac,
                },
            ..
        } => {
            let src = sources::RawFileSource::new(fs::OpenOptions::new().read(true).open(file)?);
            let maps = resolve_maps(cargo, pac, artifact)?;
            let metadata =
                recovery::Metadata::new(maps, chrono::Local::now(), *freq, comment.clone());

            Ok(Some((Box::new(src), vec![], metadata)))
        }
        ReplayOptions {
            list: true,
            trace_dir,
            ..
        } => {
            let traces = sinks::file::find_trace_files(
                trace_dir
                    .clone()
                    .unwrap_or(cargo.target_dir().join("rtic-traces")),
            )?;
            for (i, trace) in traces.enumerate() {
                let metadata =
                    sources::FileSource::new(fs::OpenOptions::new().read(true).open(&trace)?)?
                        .metadata();
                println!("{}\t{}\t{}", i, trace.display(), metadata.comment());
            }

            return Ok(None);
        }
        ReplayOptions {
            index: Some(idx),
            trace_dir,
            ..
        } => {
            let mut traces = sinks::file::find_trace_files(
                trace_dir
                    .clone()
                    .unwrap_or(cargo.target_dir().join("rtic-traces")),
            )?;
            let trace = traces
                .nth(*idx)
                .with_context(|| format!("No trace with index {}", *idx))?;

            // open trace file and print packets
            let src = sources::FileSource::new(fs::OpenOptions::new().read(true).open(&trace)?)?;
            let metadata = src.metadata();

            Ok(Some((Box::new(src), vec![], metadata)))
        }
        _ => unreachable!(),
    }
}
