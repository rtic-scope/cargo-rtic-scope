#![allow(rustdoc::bare_urls)]
#![doc = include_str!("../../docs/profile/README.md")]

use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context};
use async_std::{prelude::*, process};
use cargo_metadata::Artifact;
use chrono::Local;
use crossbeam_channel as channel;
use futures::executor::block_on;
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
mod manifest;
mod recovery;
mod sinks;
mod sources;

use build::{CargoError, CargoWrapper};
use recovery::TraceMetadata;

pub type TraceData = itm::TimestampedTracePackets;

#[derive(Debug, StructOpt)]
struct Opts {
    /// PATH, relative, or absolute path to the frontend(s) to forward
    /// recorded/replayed trace to. Tested in that order.
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

    /// Only resolve the translation maps; do not program or trace the target.
    #[structopt(long = "resolve-only")]
    resolve_only: bool,

    /// Do not attempt to flash, configure and/or reset the target:
    /// start tracing immediately.
    #[structopt(long = "dont-touch-target", requires("serial"))]
    dont_touch_target: bool,

    #[structopt(flatten)]
    pac: ManifestOptions,

    #[structopt(flatten)]
    flash_options: FlashOptions,
}

#[derive(StructOpt, Debug)]
pub struct ManifestOptions {
    /// Name of the PAC used in traced application.
    #[structopt(long = "pac-name", name = "pac-name")]
    pac_name: Option<String>,

    /// Version of the PAC used in the traced application.
    #[structopt(long = "pac-version", name = "pac-version")]
    pac_version: Option<String>,

    /// Features of the PAC used in traced application.
    #[structopt(long = "pac-features", name = "pac-features")]
    pac_features: Option<Vec<String>>,

    /// Path to PAC Interrupt enum.
    #[structopt(long = "pac-interrupt-path")]
    interrupt_path: Option<String>,

    /// Speed in Hz of the TPIU trace clock. Used to calculate
    /// timestamps of received timestamps.
    #[structopt(long = "tpiu-freq")]
    tpiu_freq: Option<u32>,

    /// Baud rate of the communication from the target TPIU.
    #[structopt(long = "tpiu-baud")]
    tpiu_baud: Option<u32>,
}

/// Replay a previously recorded trace stream for post-mortem analysis.
#[derive(StructOpt, Debug)]
struct ReplayOptions {
    #[structopt(name = "list", long = "list", short = "l")]
    list: bool,

    /// Relative path to trace file to replay.
    #[structopt(name = "trace-file", long = "trace-file")]
    trace_file: Option<PathBuf>,

    #[structopt(required_unless_one(&["list", "raw-file", "trace-file"]))]
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

    #[structopt(long = "comment", short = "c", hidden = true)]
    comment: Option<String>,
    #[structopt(flatten)]
    pac: ManifestOptions,
}

#[derive(StructOpt, Debug)]
enum Command {
    Trace(TraceOptions),
    Replay(ReplayOptions),
}

#[derive(Debug, Error)]
pub enum RTICScopeError {
    // adhoc errors
    #[error("Probe setup and/or initialization failed: {0}")]
    CommonProbeOperationError(#[from] probe_rs_cli_util::common_options::OperationError),
    #[error("I/O operation failed: {0}")]
    IOError(#[from] std::io::Error),

    // transparent errors
    #[error(transparent)]
    ManifestError(#[from] manifest::ManifestMetadataError),
    #[error(transparent)]
    MetadataError(#[from] recovery::RecoveryError),
    #[error(transparent)]
    CargoError(#[from] build::CargoError),
    #[error(transparent)]
    SourceError(#[from] sources::SourceError),
    #[error(transparent)]
    SinkError(#[from] sinks::SinkError),

    // everything else
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl diag::DiagnosableError for RTICScopeError {
    fn diagnose(&self) -> Vec<String> {
        match self {
            RTICScopeError::ManifestError(_) => vec![
                "[package.metadata.rtic-scope] takes precedence over [workspace.metadata.rtic-scope]".to_string(),
            ],
            _ => vec![],
        }
    }
}

impl RTICScopeError {
    pub fn render(&self) {
        log::err(format!("{:#?}", self)); // TODO iterator over errors instead

        // print eventual hints
        // XXX should we anyhow::Error::downcast somehow instead?
        use crate::diag::DiagnosableError;
        type DE = dyn DiagnosableError;
        for hint in self.diagnose().iter().chain(
            match self {
                Self::ManifestError(e) => Some(e as &DE),
                Self::MetadataError(e) => Some(e as &DE),
                Self::CargoError(e) => Some(e as &DE),
                Self::SourceError(e) => Some(e as &DE),
                Self::SinkError(e) => Some(e as &DE),
                _ => None,
            }
            .map(|e| e.diagnose())
            .unwrap_or_default()
            .iter(),
        ) {
            log::hint(hint.to_owned());
        }
    }
}

fn main() {
    if let Err(e) = block_on(main_try()) {
        e.render();
        std::process::exit(1); // TODO make retval depend on error type?
    }
}

// XXX This one is messy: a &mut 'a of the session is required to read
// trace data from it, but sticking it in a Box at the tail-end of trace
// requires it to be 'static.
//
// TODO remove this global
static mut SESSION: Option<probe_rs::Session> = None;

async fn main_try() -> Result<(), RTICScopeError> {
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

    // Should we quit early?
    if let Command::Trace(opts) = &opts.cmd {
        let fo = &opts.flash_options;
        fo.probe_options.maybe_load_chip_desc()?;
        if fo.early_exit(std::io::stdout())? {
            return Ok(());
        }
    }

    // Build the RTIC application to be traced in the future (not
    // necessary for some commands), and create a wrapper around cargo,
    // reusing the target directory of the application.
    #[allow(clippy::needless_question_mark)]
    let cart = async {
        log::status("Building", "RTIC target application...".to_string());
        Ok(CargoWrapper::new(
            &env::current_dir().map_err(CargoError::CurrentDirError)?,
            {
                match &opts.cmd {
                    Command::Trace(opts) => &opts.flash_options.cargo_options,
                    Command::Replay(opts) => &opts.cargo_options,
                }
            }
            .to_cargo_options(),
        )?)
    };

    // Configure source and sinks. Recover the information we need to
    // map ITM packets to RTIC tasks.
    let (source, mut sinks, metadata) = match opts.cmd {
        Command::Trace(ref opts) => match trace(opts, cart).await? {
            Some(tup) => tup,
            None => return Ok(()), // NOTE --resolve-only was passed
        },
        Command::Replay(ref opts) => {
            match replay(opts, cart).await.with_context(|| {
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

    // Spawn frontend children and get path to sockets. Create and push sinks.
    let mut children = vec![];
    for frontend in &opts.frontends {
        // Try to spawn the frontend from PATH. If that fails, try a relative path instead.
        let executables = [
            format!("rtic-scope-frontend-{}", frontend), // PATH
            format!("./{}", frontend),                   // relative
            format!("/{}", frontend),                    // absolute
        ];
        let mut child = executables
            .iter()
            .find_map(|e| {
                process::Command::new(e)
                    .stdout(process::Stdio::piped())
                    .stderr(process::Stdio::piped())
                    .spawn()
                    .ok()
            })
            .with_context(|| {
                format!(
                    "Failed to spawn a frontend child process from tested paths (PATH, relative, absolute): {:#?}",
                    executables
                )
            })?;
        {
            let socket_path = {
                async_std::io::BufReader::new(
                    child
                        .stdout
                        .take()
                        .context("Failed to pipe frontend stdout")?,
                )
                .lines()
                .next()
                .await
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

    // Wrap frontend stderrs in a poll_next wrapper such that
    // Stream::next polls the stderrs of all spawned frontends.
    let stderrs = StderrLines {
        stderrs: children
            .iter_mut()
            .map(|(_c, stderr)| async_std::io::BufReader::new(stderr).lines())
            .collect(),
        frontends: opts.frontends.clone(),
    };

    // Record the run-time duration of the below run_loop.
    let instant = std::time::Instant::now();

    // All preparatory I/O and information recovery done. Forward all
    // trace packets to all sinks.
    let stats = run_loop(source, sinks, metadata.clone(), &opts, stderrs).await;

    // Wait for frontends to proccess all packets and flush any
    // remaining stderr lines.
    //
    // TODO use StderrLines from above instead
    for (i, (child, stderr)) in children.iter_mut().enumerate() {
        let status = child.status().await;
        let mut errors = async_std::io::BufReader::new(stderr).lines();
        while let Some(err) = errors.next().await {
            log::frontend(format!(
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

    let stats = stats?;
    let duration = instant.elapsed();
    log::status(
        match opts.cmd {
            Command::Trace(_) => "Traced",
            Command::Replay(_) => "Replayed",
        },
        format!("{}.", format_status_message(&metadata, &stats, &duration)),
    );

    Ok(())
}

fn format_status_message(
    metadata: &recovery::TraceMetadata,
    stats: &Stats,
    duration: &std::time::Duration,
) -> String {
    fn format_duration(duration: &std::time::Duration) -> String {
        match duration.as_secs() {
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
        }
    }

    format!(
        "{}: {} packets processed in {time} (~{packets_per_sec:.1} packets/s; {} malformed, {} non-mappable); {sinks}",
        metadata.program_name,
        stats.packets,
        stats.malformed,
        stats.nonmappable,
        time = format_duration(duration),
        packets_per_sec = stats.packets as f32 / duration.as_secs() as f32,
        sinks = format!("{}/{} sinks operational", stats.sinks.0, stats.sinks.1),
    )
}

struct StderrLines<R>
where
    R: async_std::io::BufRead + std::marker::Unpin,
{
    pub(crate) stderrs: Vec<async_std::io::Lines<R>>,
    pub(crate) frontends: Vec<String>,
}

use async_std::pin::Pin;
use async_std::task::{self, Poll};
use futures_lite::stream::StreamExt;

impl<R> async_std::stream::Stream for StderrLines<R>
where
    R: async_std::io::BufRead + std::marker::Unpin,
{
    type Item = async_std::io::Result<String>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        for (i, stderr) in self.stderrs.iter_mut().enumerate() {
            match stderr.poll_next(cx) {
                Poll::Ready(Some(Ok(line))) => {
                    return Poll::Ready(Some(Ok(format!("{}: {}", self.frontends[i], line))))
                }
                item @ Poll::Ready(_) => return item,
                Poll::Pending => continue,
            }
        }

        task::Poll::Pending
    }
}

#[derive(Default)]
struct Stats {
    /// How many ITM packets we have received from the source.
    pub packets: usize,
    /// How many malformed ITM packets we have received from the source.
    pub malformed: usize,
    /// How many unmappable ITM packets we have received from the
    /// source.
    pub nonmappable: usize,
    /// How many sinks we started with, and how many that remained
    /// functional until the end.
    pub sinks: (usize, usize),
}

async fn run_loop<R>(
    mut source: Box<dyn sources::Source>,
    mut sinks: Vec<Box<dyn sinks::Sink>>,
    metadata: recovery::TraceMetadata,
    opts: &Opts,
    mut stderrs: StderrLines<R>,
) -> Result<Stats, RTICScopeError>
where
    R: async_std::io::BufRead + std::marker::Unpin,
{
    // Setup SIGINT handler.
    let (tx, halt) = channel::bounded(0);
    ctrlc::set_handler(move || tx.send(()).expect("Could not signal SIGINT on channel"))
        .context("Failed to install SIGINT handler")?;

    // Keep tabs on which sinks have broken during drain, if any.
    let mut sinks: Vec<(Box<dyn sinks::Sink>, bool)> =
        sinks.drain(..).map(|s| (s, false)).collect();

    let mut stats = Stats {
        sinks: (sinks.len(), sinks.len()),
        ..Stats::default()
    };

    let handle_packet = |data: TraceData,
                         stats: &mut Stats,
                         sinks: &mut Vec<(Box<dyn sinks::Sink>, bool)>|
     -> Result<(), anyhow::Error> {
        // Try to recover RTIC information for the packets.
        let chunk = metadata.build_event_chunk(data.clone());

        // Report any unmappable/unknown events that occured, and record stats
        stats.packets += data.consumed_packets;
        for event in chunk.events.iter() {
            match event {
                api::EventType::Unmappable(ref packet, ref reason) => {
                    stats.nonmappable += 1;
                    log::warn(format!(
                        "cannot map {:?} packet: {}",
                        packet, reason
                    ));
                }
                api::EventType::Unknown(ref packet) => {
                    stats.nonmappable += 1;
                    log::warn(format!(
                        "cannot map {:?} packet",
                        packet
                    ));
                }
                api::EventType::Invalid(ref malformed) => {
                    stats.malformed += 1;
                    log::warn(format!("malformed packet: {}: {:?}", malformed, malformed));
                },
                api::EventType::Overflow => log::warn("Overflow detected! Packets may have been dropped and/or timestamps will potentially be diverged until the next global timestamp.".to_string()),
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
        stats.sinks.0 = sinks.len();
        if sinks.is_empty() {
            bail!("All sinks are broken. Cannot continue.");
        }

        Ok(())
    };

    let (tx, packet) = channel::unbounded();
    let packet_poller = std::thread::spawn(move || {
        let mut buffer_warning = false;

        while let Some(data) = source.next() {
            if !buffer_warning {
                if let sources::BufferStatus::AvailWarn(avail, buf_sz) = source.avail_buffer() {
                    eprintln!(
                        "Source {} buffer is almost full ({}/{} bytes free) and it not read quickly enough",
                        source.describe(), avail, buf_sz
                    );
                    buffer_warning = true;
                }
            }

            match data {
                packet @ Ok(_) => tx.send(Some(packet)).unwrap(),
                err @ Err(_) => {
                    tx.send(Some(err)).unwrap();
                    break;
                }
            }
        }

        tx.send(None).unwrap(); // EOF
    });

    let instant = std::time::Instant::now();
    use std::time::Duration;

    loop {
        channel::select! {
            recv(packet) -> packet => match packet.unwrap() {
                Some(packet) => {
                    handle_packet(packet.context("Failed to read trace data from source")?, &mut stats, &mut sinks)?;
                },
                None => break,
            },
            recv(halt) -> _ => {
                break;
            },
            default(Duration::from_millis(100)) => (),
        }

        if let Poll::Ready(Some(error)) = futures::poll!(stderrs.next()) {
            log::frontend(error.context("Failed to read frontend stderr")?);
        }

        let duration = instant.elapsed();
        log::cont_status(
            match opts.cmd {
                Command::Trace(_) => "Tracing",
                Command::Replay(_) => "Replaying",
            },
            format!("{}...", format_status_message(&metadata, &stats, &duration)),
        );
    }

    // The thread can simply be joined in all cases except when a halt
    // is signalled during which the thread is likely to wait for the
    // next packet from source. All sinks and sources will be dropped at
    // the end of this function, so we can safely drop the thread too,
    // as we have no need for it. The program exits soon after, so we
    // can let the OS reap the thread.
    drop(packet_poller);

    Ok(stats)
}

type TraceTuple = (
    Box<dyn sources::Source>,
    Vec<Box<dyn sinks::Sink>>,
    recovery::TraceMetadata,
);

async fn trace(
    opts: &TraceOptions,
    cart: impl futures::Future<Output = Result<(CargoWrapper, Artifact), CargoError>>,
) -> Result<Option<TraceTuple>, RTICScopeError> {
    let (cargo, artifact) = cart.await?;
    let prog = format!("{} ({})", artifact.target.name, artifact.target.src_path,);
    log::status(
        "Recovering",
        format!("metadata for {}{}", prog, {
            if opts.resolve_only || opts.dont_touch_target {
                "..."
            } else {
                " and preparing target..."
            }
        }),
    );

    // Read the RTIC Scope manifest metadata block
    let manip = manifest::ManifestProperties::new(&cargo, Some(&opts.pac))?;

    // Build the translation maps
    let maps = recovery::TraceLookupMaps::from(&cargo, &artifact, &manip)?;

    if opts.resolve_only {
        println!("{:#?}", maps);
        return Ok(None);
    }

    // TODO make this into Sink::generate().remove_old(), etc.?
    let mut trace_sink = sinks::FileSink::generate_trace_file(
        &artifact,
        opts.trace_dir
            .as_ref()
            .unwrap_or(&cargo.target_dir().join("rtic-traces")),
        opts.remove_prev_traces,
    )
    .context("Failed to generate trace sink file")?;

    if !opts.dont_touch_target {
        let session = unsafe {
            SESSION = Some(
                opts.flash_options
                    .probe_options
                    .simple_attach()
                    .context("Failed to attach to target session")?,
            );

            SESSION.as_mut().unwrap()
        };

        // Flash binary to target
        let elf = artifact.executable.as_ref().unwrap();
        let flashloader = opts
            .flash_options
            .probe_options
            .build_flashloader(session, &elf.clone().into_std_path_buf())?;
        flash::run_flash_download(
            session,
            &elf.clone().into_std_path_buf(),
            &opts.flash_options,
            flashloader,
            true, // do_chip_erase
        )?;
    }

    let trace_source: Box<dyn sources::Source> = if let Some(dev) = &opts.serial {
        Box::new(sources::TTYSource::new(
            sources::tty::configure(dev, manip.tpiu_baud)
                .with_context(|| format!("Failed to configure {}", dev))?,
            &manip,
        ))
    } else {
        Box::new(sources::ProbeSource::new(
            unsafe { SESSION.as_mut().unwrap() },
            &manip,
        )?)
    };

    // Sample the timestamp of target and flush metadata to file.
    let metadata = TraceMetadata::from(
        artifact.target.name,
        maps,
        Local::now(), // XXX this is the approximate reset timestamp
        manip.tpiu_freq,
        opts.comment.clone(),
    );
    trace_sink.drain_metadata(&metadata)?;

    if !opts.dont_touch_target {
        // Reset the target device
        unsafe { SESSION.as_mut().unwrap() }
            .core(0)
            .and_then(|mut c| match opts.flash_options.reset_halt {
                true => {
                    let _ = c.reset_and_halt(std::time::Duration::from_millis(250))?;
                    Ok(())
                }
                false => c.reset(),
            })
            .map_err(sources::SourceError::ResetError)?;
    }

    log::status(
        "Recovered",
        format!(
            "{ntotal} task(s) from {prog}: {nhard} hard, {nsoft} soft.{}",
            if !opts.dont_touch_target {
                "Target reset and flashed."
            } else {
                ""
            },
            ntotal = metadata.hardware_tasks_len() + metadata.software_tasks_len(),
            prog = metadata.program_name,
            nhard = metadata.hardware_tasks_len(),
            nsoft = metadata.software_tasks_len()
        ),
    );

    Ok(Some((trace_source, vec![Box::new(trace_sink)], metadata)))
}

async fn replay(
    opts: &ReplayOptions,
    cart: impl futures::Future<Output = Result<(CargoWrapper, Artifact), CargoError>>,
) -> Result<Option<TraceTuple>, RTICScopeError> {
    match opts {
        ReplayOptions {
            raw_options:
                RawFileOptions {
                    file: Some(file),
                    comment,
                    pac,
                },
            ..
        } => {
            let (cargo, artifact) = cart.await?;
            let manip = manifest::ManifestProperties::new(&cargo, None)?;
            let src =
                sources::RawFileSource::new(fs::OpenOptions::new().read(true).open(file)?, &manip);
            let maps = recovery::TraceLookupMaps::from(&cargo, &artifact, &manip)?;
            let metadata = recovery::TraceMetadata::from(
                artifact.target.name,
                maps,
                chrono::Local::now(),
                pac.tpiu_freq.unwrap_or(manip.tpiu_freq),
                comment.clone(),
            );

            Ok(Some((Box::new(src), vec![], metadata)))
        }
        ReplayOptions {
            list: true,
            trace_dir,
            ..
        } => {
            let traces = sinks::file::find_trace_files(
                trace_dir.clone().unwrap_or(
                    cargo_metadata::MetadataCommand::new()
                        .exec()
                        .context("cargo metadata command failed")?
                        .target_directory
                        .join("rtic-traces")
                        .into(),
                ),
            )?;
            for (i, trace) in traces.enumerate() {
                let metadata =
                    sources::FileSource::new(fs::OpenOptions::new().read(true).open(&trace)?)?
                        .metadata();
                println!("{}\t{}\t{:?}", i, trace.display(), metadata.comment);
            }

            Ok(None)
        }
        ReplayOptions {
            trace_file: Some(file),
            ..
        } => {
            let src = sources::FileSource::new(fs::OpenOptions::new().read(true).open(&file)?)?;
            let metadata = src.metadata();
            Ok(Some((Box::new(src), vec![], metadata)))
        }
        ReplayOptions {
            index: Some(idx),
            trace_dir,
            ..
        } => {
            let mut traces = sinks::file::find_trace_files(
                trace_dir.clone().unwrap_or(
                    cargo_metadata::MetadataCommand::new()
                        .exec()
                        .context("cargo metadata command failed")?
                        .target_directory
                        .join("rtic-traces")
                        .into(),
                ),
            )?;
            let trace = traces
                .nth(*idx)
                .with_context(|| format!("No trace with index {}", *idx))?;

            let src = sources::FileSource::new(fs::OpenOptions::new().read(true).open(&trace)?)?;
            let metadata = src.metadata();

            Ok(Some((Box::new(src), vec![], metadata)))
        }
        _ => unreachable!(),
    }
}
