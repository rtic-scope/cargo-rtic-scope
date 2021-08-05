use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{anyhow, Context, Result};
use cargo_metadata::Artifact;
use colored::Colorize;
use probe_rs_cli_util::{
    argument_handling,
    common_options::{self, cargo_help_message, FlashOptions},
    flash,
};
use structopt::StructOpt;

mod build;
mod pacp;
mod recovery;
mod sinks;
mod sources;

use build::CargoWrapper;

pub type TraceData = Result<itm_decode::TimestampedTracePackets, itm_decode::MalformedPacket>;

#[derive(Debug, StructOpt)]
struct Opts {
    /// The frontend to forward recorded/replayed trace to.
    #[structopt(long = "frontend", short = "-F", default_value = "dummy")]
    frontends: Vec<String>,

    #[structopt(subcommand)]
    cmd: Command,
}

impl Opts {
    pub const ARGUMENTS: &'static [&'static str] = &["frontend="];
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

    _cargo_args: Vec<String>,
}

impl TraceOptions {
    pub const ARGUMENTS: &'static [&'static str] = &[
        "serial=",
        "trace-dir=",
        "comment=",
        "clear-traces",
        "pac=",
        "pac-features=",
        "pac-interrupt-path=",
    ];
#[derive(StructOpt, Debug)]
pub struct PACOptions {
    /// Name of the PAC used in traced application.
    #[structopt(long = "pac", name = "pac")]
    name: Option<String>,

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

impl TPIUOptions {
    pub const ARGUMENTS: &'static [&'static str] = &["tpiu-freq=", "tpiu-baud="];
}

/// Replay a previously recorded trace stream for post-mortem analysis.
#[derive(StructOpt, Debug)]
struct ReplayOptions {
    #[structopt(name = "list", long = "list", short = "l")]
    list: bool,

    #[structopt(required_unless("list"))]
    index: Option<usize>,

    /// Directory where previously recorded trace streams. By default,
    /// the build cache of <bin> is used (usually ./target/).
    #[structopt(name = "trace-dir", long = "trace-dir", parse(from_os_str))]
    trace_dir: Option<PathBuf>,

    _cargo_args: Vec<String>,
}

impl ReplayOptions {
    pub const ARGUMENTS: &'static [&'static str] = &["list", "index=", "trace-dir="];
}

#[derive(StructOpt, Debug)]
enum Command {
    #[structopt(setting = structopt::clap::AppSettings::TrailingVarArg)]
    #[structopt(setting = structopt::clap::AppSettings::AllowLeadingHyphen)]
    Trace(TraceOptions),
    #[structopt(setting = structopt::clap::AppSettings::TrailingVarArg)]
    #[structopt(setting = structopt::clap::AppSettings::AllowLeadingHyphen)]
    Replay(ReplayOptions),
}

fn main() -> Result<()> {
    // Handle CLI options
    let mut args: Vec<_> = std::env::args().collect();
    // When called by cargo, first argument will be "rtic-scope".
    if args.get(1) == Some(&"rtic-scope".to_string()) {
        args.remove(1);
    }
    let matches = Opts::clap()
        .after_help(cargo_help_message("cargo rtic-scope trace").as_str())
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

    // Remove arguments not to be forwarded to cargo
    args.remove(0); // remove executable path
    args.remove(0); // remove subcommand
    argument_handling::remove_arguments(
        &Opts::ARGUMENTS
            .iter()
            .chain(TraceOptions::ARGUMENTS)
            .chain(TPIUOptions::ARGUMENTS)
            .chain(ReplayOptions::ARGUMENTS)
            .copied()
            .chain(common_options::common_arguments())
            .collect::<Vec<&str>>(),
        &mut args,
    );

    // Build RTIC application to be traced, and create a wrapper around
    // cargo, reusing the target directory of the application.
    let (cargo, artifact) = CargoWrapper::new(&env::current_dir()?, args)?;

    // Setup SIGINT handler
    let halt = Arc::new(AtomicBool::new(false));
    let h = halt.clone();
    ctrlc::set_handler(move || {
        h.store(true, Ordering::SeqCst);
    })
    .context("Failed to install SIGINT handler")?;

    eprintln!(
        "  {} metadata for {prog} and preparing target...",
        "Recovering".green().bold(),
        prog = format!(
            "{} ({})",
            artifact.target.name,
            artifact.target.src_path.display()
        ),
    );

    // Configure source and sinks. Recover the information we need to
    // map IRQ numbers to RTIC tasks.
    let (mut source, mut sinks, metadata) = match opts.cmd {
        Command::Trace(ref opts) => match trace(opts, &cargo, &artifact)? {
            Some(tup) => tup,
            None => return Ok(()),
        },
        Command::Replay(ref opts) => {
            match replay(opts, &cargo).with_context(|| {
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

    eprintln!(
        "   {} {ntotal} task(s) from {prog}: {nhard} hard, {nsoft} soft. Target reset.",
        "Recovered".green().bold(),
        ntotal = metadata.hardware_tasks() + metadata.software_tasks(),
        prog = artifact.target.name,
        nhard = metadata.hardware_tasks(),
        nsoft = metadata.software_tasks(),
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
            sinks.push(Box::new(sinks::FrontendSink::new(socket, metadata.clone())));
        }

        let stderr = child
            .stderr
            .take()
            .context("Failed to take frontend stderr")?;
        children.push((child, stderr));
    }

    if let sources::BufferStatus::Unknown = source.avail_buffer() {
        eprintln!(
            "{}: buffer size of source {} could not be found; buffer may overflow and corrupt trace stream without further warning",
            "warning".yellow().bold(),
            source.describe(),
        );
    }

    eprintln!(
        "    {} {}...",
        match &opts.cmd {
            Command::Trace(_) => "Tracing",
            Command::Replay(_) => "Replaying",
        }
        .green()
        .bold(),
        artifact.target.name,
    );

    // All preparatory I/O and information recovery done. Forward all
    // trace packets to all sinks.
    //
    // Keep tabs on which sinks have broken during drain, if any.
    let mut sinks: Vec<(Box<dyn sinks::Sink>, bool)> =
        sinks.drain(..).map(|s| (s, false)).collect();
    let mut retstatus = Ok(());
    let mut buffer_warning = false;
    while let Some(data) = source.next() {
        if halt.load(Ordering::SeqCst) {
            break;
        }

        let data = data.with_context(|| {
            format!(
                "Failed to read trace data from source {}",
                source.describe()
            )
        })?;

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

        if let Err(ref malformed) = data {
            eprintln!(
                "{}: failed to decode an ITM packet: {:?}",
                "warning".yellow().bold(),
                malformed, // TODO thiserror
            );
        }

        for (sink, is_broken) in sinks.iter_mut() {
            if let Err(e) = sink.drain(data.clone()) {
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
            retstatus = Err(anyhow!("All sinks broken. Cannot continue."));
            break;
        }
    }

    // close frontend sockets
    drop(sinks);

    // Wait for frontends to proccess all packets and echo its' stderr
    for (i, (child, stderr)) in children.iter_mut().enumerate() {
        let status = child.wait();
        for err in BufReader::new(stderr).lines() {
            eprintln!(
                "{}: {}",
                opts.frontends.get(i).unwrap(),
                err.context("Failed to read frontend stderr")?
            );
        }
        if let Err(err) = status {
            eprintln!(
                "Frontend {} exited with error: {}",
                opts.frontends.get(i).unwrap(),
                err
            );
        }
    }

    retstatus
}

type TraceTuple = (
    Box<dyn sources::Source>,
    Vec<Box<dyn sinks::Sink>>,
    recovery::Metadata,
);

fn trace(
    opts: &TraceOptions,
    cargo: &CargoWrapper,
    artifact: &Artifact,
) -> Result<Option<TraceTuple>> {
    let mut session = opts
        .flash_options
        .probe_options
        .simple_attach()
        .context("Faled to attach to target session")?;

    // TODO make this into Sink::generate().remove_old(), etc.
    let mut trace_sink = sinks::FileSink::generate_trace_file(
        &artifact,
        opts.trace_dir
            .as_ref()
            .unwrap_or(&cargo.target_dir().join("rtic-traces")),
        opts.remove_prev_traces,
    )
    .context("Failed to generate trace sink file")?;

    // Find crate name, features and path to interrupt enum from
    // manifest metadata, or override options.
    let pacp = pacp::PACProperties::new(&cargo, &opts.pac)
        .context("Failed to complete PAC propeties from manifest metadata")?;

    // Map IRQ numbers and DWT matches to their respective RTIC tasks
    let maps = recovery::TaskResolver::new(&artifact, &cargo, pacp)
        .context("Failed to parse RTIC application source file")?
        .resolve()
        .context("Failed to resolve tasks")?;

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

fn replay(opts: &ReplayOptions, cargo: &CargoWrapper) -> Result<Option<TraceTuple>> {
    let mut traces = sinks::file::find_trace_files({
        if let Some(ref dir) = opts.trace_dir {
            dir.to_path_buf()
        } else {
            cargo.target_dir().join("rtic-traces")
        }
    })?;

    if opts.list {
        for (i, trace) in traces.enumerate() {
            let metadata =
                sources::FileSource::new(fs::OpenOptions::new().read(true).open(&trace)?)?
                    .metadata();
            println!("{}\t{}\t{}", i, trace.display(), metadata.comment());
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
