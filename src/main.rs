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
use structopt::StructOpt;
use probe_rs::{
    config::{RegistryError, TargetSelector},
    flashing::{
        self,
        DownloadOptions, FileDownloadError, FlashError, FlashProgress, ProgressEvent,
    },
    DebugProbeError, DebugProbeSelector, Probe, Session, Target, WireProtocol,
};
use probe_rs_cli_util::log;

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

#[derive(StructOpt, Debug)]
enum Command {
    Trace(TraceOpts),
    Replay(ReplayOpts),
}

/// Execute and trace a chosen application on a target device and record
/// the trace stream to file.
#[derive(StructOpt, Debug)]
struct TraceOpts {
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

    /// Optional override of the trace clock frequency.
    #[structopt(long = "freq")]
    trace_clk_freq: Option<u32>,

    #[structopt(flatten)]
    flash_options: FlashOpts,
}

#[derive(Debug, StructOpt)]
// #[structopt(bin_name = "cargo flash", after_help = CARGO_HELP_MESSAGE)]
struct FlashOpts {
    // #[structopt(short = "V", long = "version")]
    // pub version: bool,
    #[structopt(name = "chip", long = "chip")]
    chip: Option<String>,
    #[structopt(name = "chip description file path", long = "chip-description-path")]
    chip_description_path: Option<String>,
    #[structopt(name = "list-chips", long = "list-chips")]
    list_chips: bool,
    #[structopt(
        name = "list-probes",
        long = "list-probes",
        help = "Lists all the connected probes that can be seen.\n\
        If udev rules or permissions are wrong, some probes might not be listed."
    )]
    list_probes: bool,
    #[structopt(name = "disable-progressbars", long = "disable-progressbars")]
    disable_progressbars: bool,
    #[structopt(name = "protocol", long = "protocol", default_value = "swd")]
    protocol: WireProtocol,
    #[structopt(
        long = "probe",
        help = "Use this flag to select a specific probe in the list.\n\
        Use '--probe VID:PID' or '--probe VID:PID:Serial' if you have more than one probe with the same VID:PID."
    )]
    probe_selector: Option<DebugProbeSelector>,
    #[structopt(
        long = "connect-under-reset",
        help = "Use this flag to assert the nreset & ntrst pins during attaching the probe to the chip."
    )]
    connect_under_reset: bool,
    #[structopt(
        name = "reset-halt",
        long = "reset-halt",
        help = "Use this flag to reset and halt (instead of just a reset) the attached core after flashing the target."
    )]
    reset_halt: bool,
    #[structopt(
        name = "level",
        long = "log",
        help = "Use this flag to set the log level.\n\
        Default is `warning`. Possible choices are [error, warning, info, debug, trace]."
    )]
    log: Option<log::Level>,
    #[structopt(name = "speed", long = "speed", help = "The protocol speed in kHz.")]
    speed: Option<u32>,
    #[structopt(
        name = "restore-unwritten",
        long = "restore-unwritten",
        help = "Enable this flag to restore all bytes erased in the sector erase but not overwritten by any page."
    )]
    restore_unwritten: bool,
    #[structopt(
        name = "filename",
        long = "flash-layout",
        help = "Requests the flash builder to output the layout into the given file in SVG format."
    )]
    flash_layout_output_path: Option<String>,
    #[structopt(
        name = "elf file",
        long = "elf",
        help = "The path to the ELF file to be flashed."
    )]
    elf: Option<String>,
    #[structopt(
        name = "directory",
        long = "work-dir",
        help = "The work directory from which cargo-flash should operate from."
    )]
    work_dir: Option<String>,

    #[structopt(long = "dry-run")]
    dry_run: bool,

    #[structopt(flatten)]
    /// Arguments which are forwarded to 'cargo build'.
    cargo_options: CargoOptions,
}

impl FlashOpts {
    pub fn as_options(&self) -> Vec<String> {
        let mut opts = vec![];

        if let Some(chip) = &self.chip {
            opts.push("--chip".into());
            opts.push(format!("{}", chip));
        }
        if let Some(path) = &self.chip_description_path {
            opts.push(format!("--chip-description-path {}", path));
        }
        if self.list_chips {
            opts.push("--list-chips".into());
        }
        if self.list_probes {
            opts.push("--list-probes".into());
        }
        if self.disable_progressbars {
            opts.push("--disable-progressbars".into());
        }
        opts.push("--protocol".into());
        opts.push(format!("{:?}", self.protocol));
        if let Some(probe) = &self.probe_selector {
            opts.push(format!("--probe {}", if let Some(serial) = &probe.serial_number {
                format!("{}:{}:{}", probe.vendor_id, probe.product_id, serial)
            } else {
                format!("{}:{}", probe.vendor_id, probe.product_id)
            }));
        }
        if self.connect_under_reset {
            opts.push("--connect-under-reset".into());
        }
        if self.reset_halt {
            opts.push("--reset-halt".into());
        }
        if let Some(level) = &self.log {
            opts.push(format!("--log {:?}", level));
        }
        if let Some(speed) = &self.speed {
            opts.push(format!("--speed {}", speed));
        }
        if self.restore_unwritten {
            opts.push("--restore-unwritten".into());
        }
        if let Some(file_name) = &self.flash_layout_output_path {
            todo!();
        }

        // TODO fill in remainer

        opts
    }
}

#[derive(StructOpt, Debug)]
struct CargoOptions {
    #[structopt(name = "binary", long = "bin", hidden = true)]
    bin: Option<String>,
    #[structopt(name = "example", long = "example", hidden = true)]
    example: Option<String>,
    #[structopt(name = "package", short = "p", long = "package", hidden = true)]
    package: Option<String>,
    #[structopt(name = "release", long = "release", hidden = true)]
    release: bool,
    #[structopt(name = "target", long = "target", hidden = true)]
    target: Option<String>,
    #[structopt(
        name = "PATH",
        long = "manifest-path",
        parse(from_os_str),
        hidden = true
    )]
    manifest_path: Option<PathBuf>,
    #[structopt(long, hidden = true)]
    no_default_features: bool,
    #[structopt(long, hidden = true)]
    all_features: bool,
    #[structopt(long, hidden = true)]
    features: Vec<String>,
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
    let mut drain_status = Ok(());
    for packets in source.into_iter() {
        if halt.load(Ordering::SeqCst) {
            break;
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

        // remove all broken sinks
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
    let mut trace_tty = sources::TTYSource::new(
        sources::tty::configure(&opts.serial)
            .with_context(|| format!("Failed to configure {}", opts.serial))?,
    );

    let (cargo, artifact) = build_target_binary(&opts.flash_options.cargo_options.bin.as_ref().unwrap(), vec![])?;

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

    // Flash artifact to target with cargo-flash
    {
        let mut cargo_flash = std::process::Command::new("cargo");
        cargo_flash.arg("flash")
            .args(opts.flash_options.as_options())
            .args(&["--elf", &artifact.executable.unwrap().to_str().unwrap()]);
        let mut child = cargo_flash.spawn().context("Failed to execute cargo-flash. Is it installed?")?;
        child.wait().context("cargo-flash exited non-zero")?;
    }

    // Re-establish probe connection
    let probes = Probe::list_all();
    if probes.is_empty() {
        bail!("No supported target probes found");
    }
    let probe = probes[0].open().context("Unable to open first probe")?;
    let mut session = probe
        .attach("stm32f401re")
        .context("Failed to attach to stm32f401re")?;

    // Sample the timestamp of target reset, wait for trace clock
    // frequency payload, flush metadata to file.
    let metadata = trace_sink
        .init(maps, opts.trace_clk_freq, || {
            // Reset the target to  execute flashed binary
            eprintln!("Resetting target...");
            let mut core = session.core(0)?;
            core.reset().context("Unable to reset target")?;
            eprintln!("Reset.");

            // Wait for a non-zero trace clock frequency payload.
            //
            // NOTE A side-effect here is that we effectively disregard all
            // packets that are emitted during target initialization. In
            // local tests these packets have been but noise, so this is
            // okay for the moment.
            let freq = sources::wait_for_trace_clk_freq(&mut trace_tty)
                .context("Failed to read trace clock frequency from source")?;

            Ok(freq)
        })
        .context("Failed to initialize metadata")?;

    Ok(Some((
        Box::new(trace_tty),
        vec![Box::new(trace_sink)],
        metadata,
    )))
}

fn replay(opts: &ReplayOpts) -> Result<Option<TraceTuple>> {
    let mut traces = sinks::file::find_trace_files({
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
