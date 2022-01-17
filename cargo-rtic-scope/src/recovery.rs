//! Module responsible for recovering RTIC application metadata to
//! associate ITM packets with RTIC tasks.
use crate::build::{self, CargoWrapper};
use crate::diag;
use crate::manifest::ManifestProperties;

use std::fs;
use std::io::Write;
use std::iter::FromIterator;

use cargo_metadata::Artifact;
use chrono::Local;
use include_dir::{dir::ExtractMode, include_dir};
use itm::{ExceptionAction, MemoryAccessType, TimestampedTracePackets, TracePacket, VectActive};

use indexmap::{IndexMap, IndexSet};
use proc_macro2::{TokenStream, TokenTree};
use quote::{format_ident, quote};
use rtic_scope_api::{EventChunk, EventType, TaskAction};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("The DataTraceValue {0:?} does not map to any software task")]
    MissingSoftwareMapping(usize),
    #[error("The DataTraceValue {0:#?} is not a valid payload")]
    InvalidSoftwareValue(Vec<u8>),
    #[error("The IRQ {0:?} does not map to any hardware task or software task dispatcher")]
    MissingHardwareMapping(VectActive),
    #[error("Failed to read artifact source file: {0}")]
    SourceRead(#[source] std::io::Error),
    #[error("Failed to tokenize artifact source file: {0}")]
    TokenizeFail(#[source] syn::Error),
    #[error("Failed to find arguments to RTIC application")]
    RTICArgumentsMissing,
    #[error("Failed to parse the content of the RTIC application")]
    RTICParseFail(#[source] syn::Error),
    #[error("Failed to extract and/or configure the intermediate crate directory to disk: {0}")]
    LibExtractFail(#[source] std::io::Error),
    #[error("Failed to build the intermediate crate: {0}")]
    LibBuildFail(#[from] build::CargoError),
    #[error("Failed to load the intermediate shared object: {0}")]
    LibLoadFail(#[source] libloading::Error),
    #[error("Failed to lookup symbol in the intermediate shared object: {0}")]
    LibLookupFail(#[source] libloading::Error),
}

impl diag::DiagnosableError for RecoveryError {
    fn diagnose(&self) -> Vec<String> {
        match self {
            RecoveryError::RTICArgumentsMissing => vec![
                "RTIC Scope expects an RTIC application declaration on the form `#[rtic::app(...)] mod app { ... }` where the first `...` is the application arguments.".to_string(),
            ],
            RecoveryError::InvalidSoftwareValue(_) => vec![
                "Invalid DataTraceValue payloads are those of zero length or with non-zero subsequent bytes (only the first byte may be non-zero).".to_string(),
                "RTIC Scope supports up to 255 software tasks at the present.".to_string(),
            ],
            _ => vec![],
        }
    }
}

/// Lookup maps for hardware and software tasks.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TraceLookupMaps {
    software: SoftwareMap,
    hardware: HardwareMap,
}

impl TraceLookupMaps {
    pub fn from(
        cargo: &CargoWrapper,
        artifact: &Artifact,
        manip: &ManifestProperties,
    ) -> Result<Self, RecoveryError> {
        // Parse the RTIC app from the source code and analyze it via
        // rtic-syntax.
        let src = syn::parse_str::<TokenStream>(
            &fs::read_to_string(artifact.target.src_path.as_std_path())
                .map_err(RecoveryError::SourceRead)?,
        )
        .map_err(RecoveryError::TokenizeFail)?;
        let (app, ast) = Self::parse_rtic_app(src)?;

        Ok(Self {
            software: SoftwareMap::from(&app, ast, manip, cargo)?,
            hardware: HardwareMap::from(&app, cargo, manip)?,
        })
    }

    fn parse_rtic_app(
        src: TokenStream,
    ) -> Result<(rtic_syntax::P<rtic_syntax::ast::App>, TokenStream), RecoveryError> {
        // iterate over the tokenstream until we find #[app(...)] mod app { ... }
        let mut rtic_app = src.into_iter().skip_while(|token| {
            if let TokenTree::Group(g) = token {
                let mut stream = g.stream().into_iter();
                if let (Some(c1), Some(c2)) = (stream.next(), stream.nth(2)) {
                    match (c1.to_string().as_str(), c2.to_string().as_str()) {
                        ("rtic", "app") => return false,
                        _ => return true,
                    }
                }
            }
            true
        });
        // extract the arguments in #[app(...)]
        let arguments = {
            let mut args: Option<TokenStream> = None;
            if let Some(TokenTree::Group(g)) = rtic_app.next() {
                if let Some(TokenTree::Group(g)) = g.stream().into_iter().nth(4) {
                    args = Some(g.stream());
                }
            }
            args.ok_or(RecoveryError::RTICArgumentsMissing)?
        };
        let ast = rtic_app.collect::<TokenStream>();

        // parse the found tokenstreams
        let (app, _analysis) = {
            let mut settings = rtic_syntax::Settings::default();
            settings.parse_binds = true;
            rtic_syntax::parse2(arguments, ast.clone(), settings)
                .map_err(RecoveryError::RTICParseFail)?
        };
        Ok((app, ast))
    }

    pub fn resolve_hardware_task(
        &self,
        veca: &VectActive,
    ) -> Result<Option<String>, RecoveryError> {
        if self.software.task_dispatchers.contains(veca) {
            return Ok(None);
        }

        Ok(Some(
            self.hardware
                .0
                .get(veca)
                .ok_or_else(|| RecoveryError::MissingHardwareMapping(veca.to_owned()))?
                .join("::"),
        ))
    }

    pub fn resolve_software_task(
        &self,
        comp: &u8,
        value: &[u8],
    ) -> Result<Option<EventType>, RecoveryError> {
        if let Some(action) = self.software.comparators.get(&(*comp as usize)) {
            if value.len() != 1 {
                return Err(RecoveryError::InvalidSoftwareValue(value.to_owned()));
            }
            let value = value[0] as usize;

            let name = self
                .software
                .map
                .get(&value)
                .ok_or(RecoveryError::MissingSoftwareMapping(value))?
                .join("::");

            Ok(Some(EventType::Task {
                name,
                action: action.to_owned(),
            }))
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct SoftwareMap {
    pub task_dispatchers: IndexSet<VectActive>,
    #[serde(with = "vectorize")]
    pub comparators: IndexMap<usize, TaskAction>,
    #[serde(with = "vectorize")]
    pub map: IndexMap<usize, Vec<String>>,
}
impl SoftwareMap {
    pub fn from(
        app: &rtic_syntax::ast::App,
        ast: TokenStream,
        manip: &ManifestProperties,
        cargo: &CargoWrapper,
    ) -> Result<Self, RecoveryError> {
        let actions = [
            (manip.dwt_enter_id, TaskAction::Entered),
            (manip.dwt_exit_id, TaskAction::Exited),
        ];
        let map = Self::parse_ast(ast);

        // Extract all dispatcher interrupt idents from #[app(..,
        // dispatchers = [..])] and resolve the associated VectActive.
        let task_dispatchers: IndexSet<VectActive> = resolve_int_nrs(
            cargo,
            manip,
            app.args
                .extern_interrupts
                .iter()
                .map(|(ident, _ext_int_attrs)| ident.to_string())
                .collect(),
        )?
        .values()
        .cloned()
        .collect();

        Ok(Self {
            task_dispatchers,
            comparators: IndexMap::from_iter(actions.iter().cloned()),
            map,
        })
    }

    fn parse_ast(app: TokenStream) -> IndexMap<usize, Vec<String>> {
        struct TaskIDGenerator(usize);
        impl TaskIDGenerator {
            pub fn new() -> Self {
                TaskIDGenerator(0)
            }

            /// Generate a unique task id. Returned values mirror the behavior
            /// of the `trace`-macro from the tracing module.
            pub fn generate(&mut self) -> usize {
                let id = self.0;
                self.0 += 1;
                id
            }
        }

        let app = syn::parse2::<syn::Item>(app).unwrap();
        let mut ctx: Vec<syn::Ident> = vec![];
        let mut assocs = IndexMap::<usize, Vec<String>>::new();
        let mut id_gen = TaskIDGenerator::new();

        fn traverse_item(
            item: &syn::Item,
            ctx: &mut Vec<syn::Ident>,
            assocs: &mut IndexMap<usize, Vec<String>>,
            id_gen: &mut TaskIDGenerator,
        ) {
            match item {
                // handle
                //
                //   #[trace]
                //   fn fun() {
                //       #[trace]
                //       fn sub_fun() {
                //           // ...
                //       }
                //   }
                //
                syn::Item::Fn(fun) => {
                    // record the full path of the function
                    ctx.push(fun.sig.ident.clone());

                    // is the function decorated with #[trace]?
                    if fun.attrs.iter().any(|a| a.path == syn::parse_quote!(trace)) {
                        assocs.insert(
                            id_gen.generate(),
                            ctx.iter().map(|i| i.to_string()).collect(),
                        );
                    }

                    // walk down all other nested functions
                    for item in fun.block.stmts.iter().filter_map(|stmt| match stmt {
                        syn::Stmt::Item(item) => Some(item),
                        _ => None,
                    }) {
                        traverse_item(item, ctx, assocs, id_gen);
                    }

                    // we've handled with function, return to upper scope
                    ctx.pop();
                }
                // handle
                //
                //   mod scope {
                //       #[trace]
                //       fn fun() {
                //           // ...
                //       }
                //   }
                //
                syn::Item::Mod(m) => {
                    ctx.push(m.ident.clone());
                    if let Some((_, items)) = &m.content {
                        for item in items {
                            traverse_item(item, ctx, assocs, id_gen);
                        }
                    }
                    ctx.pop();
                }
                _ => (),
            }
        }

        traverse_item(&app, &mut ctx, &mut assocs, &mut id_gen);

        assocs
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct HardwareMap(#[serde(with = "vectorize")] IndexMap<VectActive, Vec<String>>);
impl HardwareMap {
    pub fn from(
        app: &rtic_syntax::ast::App,
        cargo: &CargoWrapper,
        manip: &ManifestProperties,
    ) -> Result<Self, RecoveryError> {
        use cortex_m::peripheral::scb::Exception;
        macro_rules! resolve_core_interrupts {
            ($($excpt:ident),+) => {{
                [$({
                    let exception = Exception::$excpt;
                    (format!("{:?}", exception), exception)
                },)+]
            }}
        }
        // Exceptions common to all ARMv7-M targets. Known as /processor
        // core exceptions/ or /internal interrupts/ These exceptions
        // will be received over ITM as-is, and no additional
        // information need to be recovered to use them. These labels
        // are the same ones one can bind hardware tasks to, e.g.
        //
        //    #[task(binds = SysTick)]
        //    fn task(_: task::Context) {}
        //
        // This list is sourced from the ARMv7-M arch. reference manual,
        // table B1-4.
        let internal_ints: IndexMap<String, Exception> = IndexMap::from_iter(
            resolve_core_interrupts!(
                NonMaskableInt,
                HardFault,
                MemoryManagement,
                BusFault,
                UsageFault,
                SecureFault,
                SVCall,
                DebugMonitor,
                PendSV,
                SysTick
            )
            .iter()
            .cloned(),
        );

        // Find all bound exceptions from the #[task(bound = ...)]
        // arguments in the now-parsed source file. Partition internal
        // (see above) and external interrupts. Further recovery work is
        // required for the external interrupts: over ITM we'll receive
        // the IRQ number which we need to associate to a label (found
        // in PAC::Interrupt).
        type TaskBindMaps = IndexMap<String, String>;
        let (known_maps, unknown_maps): (TaskBindMaps, TaskBindMaps) = app
            .hardware_tasks
            .iter()
            // Find (interrupt name, task name) associations.
            .map(|(task_name, hwt)| (hwt.args.binds.to_string(), task_name.to_string()))
            // Separate core interrupts from device-specific interrupts
            .partition(|(bind, _)| internal_ints.contains_key(bind));
        let mut known_maps = known_maps
            .iter()
            .map(|(bind, task_name)| {
                (
                    VectActive::Exception(*internal_ints.get(bind).unwrap()),
                    vec!["app".to_string(), task_name.to_owned()],
                )
            })
            .collect();

        if unknown_maps.is_empty() {
            return Ok(Self(known_maps));
        }

        // Resolve unknown maps by help of a cdylib; extend the known
        // map collection.
        let resolved_maps: IndexMap<VectActive, Vec<String>> = resolve_int_nrs(
            cargo,
            manip,
            unknown_maps.iter().map(|(k, _v)| k.to_owned()).collect(),
        )?
        .iter()
        .map(|(bind, irqn)| {
            (
                irqn.to_owned(),
                vec![
                    "app".to_string(),
                    unknown_maps.get(bind).unwrap().to_owned(),
                ],
            )
        })
        .collect();
        known_maps.extend(resolved_maps);

        Ok(Self(known_maps))
    }
}

fn resolve_int_nrs(
    cargo: &CargoWrapper,
    pacp: &ManifestProperties,
    binds: Vec<String>,
) -> Result<IndexMap<String, VectActive>, RecoveryError> {
    const ADHOC_FUNC_PREFIX: &str = "rtic_scope_func_";

    // Extract adhoc source to a temporary directory and apply adhoc
    // modifications.
    let target_dir = cargo.target_dir().join("cargo-rtic-trace-libadhoc");
    include_dir!("assets/libadhoc")
        .extract(&target_dir, ExtractMode::Overwrite)
        .map_err(RecoveryError::LibExtractFail)?;
    // NOTE See <https://github.com/rust-lang/cargo/issues/9643>
    fs::rename(
        target_dir.join("not-Cargo.toml"),
        target_dir.join("Cargo.toml"),
    )
    .map_err(RecoveryError::LibExtractFail)?;
    // Add required crate (and optional feature) as dependency
    {
        let mut manifest = fs::OpenOptions::new()
            .append(true)
            .open(target_dir.join("Cargo.toml"))
            .map_err(RecoveryError::LibExtractFail)?;
        let dep = format!(
            "\n{} = {{ version = \"{}\", features = [{}]}}\n",
            pacp.pac_name,
            pacp.pac_version,
            pacp.pac_features
                .iter()
                .map(|f| format!("\"{}\"", f))
                .collect::<Vec<String>>()
                .join(","),
        );
        manifest
            .write_all(dep.as_bytes())
            .map_err(RecoveryError::LibExtractFail)?;
    }
    // Prepare lib.rs
    {
        // Import PAC::Interrupt
        let mut src = fs::OpenOptions::new()
            .append(true)
            .open(target_dir.join("src/lib.rs"))
            .map_err(RecoveryError::LibExtractFail)?;
        let import = str::parse::<TokenStream>(&pacp.interrupt_path)
            .expect("Failed to tokenize pacp.interrupt_path");
        let import = quote!(use #import;);
        src.write_all(format!("\n{}\n", import).as_bytes())
            .map_err(RecoveryError::LibExtractFail)?;

        // Generate the functions that must be exported
        for bind in &binds {
            let fun = format_ident!("{}{}", ADHOC_FUNC_PREFIX, bind);
            let int_ident = format_ident!("{}", bind);
            let fun = quote!(
                #[no_mangle]
                pub extern fn #fun() -> u16 {
                    Interrupt::#int_ident.number()
                }
            );
            src.write_all(format!("\n{}\n", fun).as_bytes())
                .map_err(RecoveryError::LibExtractFail)?;
        }
    }

    // Build the adhoc library, load it, and resolve all exception idents
    let artifact = cargo.build(
        &target_dir,
        // Host target triple need not be specified when CARGO is set.
        None,
        "cdylib",
    )?;
    let lib = unsafe {
        libloading::Library::new(artifact.filenames.first().unwrap())
            .map_err(RecoveryError::LibLoadFail)?
    };
    let binds: Result<Vec<(String, VectActive)>, RecoveryError> = binds
        .iter()
        .map(|b| {
            let func: libloading::Symbol<extern "C" fn() -> u16> = unsafe {
                lib.get(format!("{}{}", ADHOC_FUNC_PREFIX, b).as_bytes())
                    .map_err(RecoveryError::LibLookupFail)?
            };

            // Convert the IRQn to a VectActive.
            //
            // The offset denotes at what offset from the start of the
            // interrupt vector external (device-specific) interrupts
            // are enumerated. cortex_m::interrupt::InterruptNumber
            // (used above) enumerates starting at this offset so we
            // must compensate. See also B1.5.2 in the ARMv7-M
            // Architecture Reference Manual.
            const DEVICE_INTERRUPTS_OFFSET: u16 = 16;
            let irqn =
                VectActive::from(func() + DEVICE_INTERRUPTS_OFFSET).expect("Invalid/reserved IRQn");

            Ok((b.to_string(), irqn))
        })
        .collect();
    Ok(binds?.iter().cloned().collect())
}

/// Contains all metadata for a single trace.
#[derive(Clone, Serialize, Deserialize)]
pub struct TraceMetadata {
    /// Name of the RTIC application that was/is traced.
    pub program_name: String,

    /// Lookup maps for data received over ITM to RTIC application idents.
    maps: TraceLookupMaps,

    /// Timestamp of target reset, after which tracing begins.
    ///
    /// Note: this timestamp is sampled host-side and is approximate.
    reset_timestamp: chrono::DateTime<Local>,

    /// Frequency of the target TPIU clock. Used to generate absolute
    /// timestamps. Set via `tpiu_freq` in
    /// `[{package,workspace}.metadata.rtic-scope]` from `Cargo.toml` or
    /// overridden via the `--tpiu-freq` trace option.
    tpiu_freq: u32,

    /// Optional comment of this particular trace.
    pub comment: Option<String>,
}

impl TraceMetadata {
    pub fn from(
        program_name: String,
        maps: TraceLookupMaps,
        reset_timestamp: chrono::DateTime<Local>,
        tpiu_freq: u32,
        comment: Option<String>,
    ) -> Self {
        Self {
            program_name,
            maps,
            reset_timestamp,
            tpiu_freq,
            comment,
        }
    }

    pub fn hardware_tasks_len(&self) -> usize {
        self.maps.hardware.0.len()
    }

    pub fn software_tasks_len(&self) -> usize {
        self.maps.software.map.len()
    }

    pub fn build_event_chunk(
        &self,
        TimestampedTracePackets {
            timestamp,
            packets,
            malformed_packets,
            consumed_packets: _,
        }: TimestampedTracePackets,
    ) -> EventChunk {
        let mut events = vec![];
        for packet in packets.iter() {
            match packet {
                TracePacket::Sync => (), // NOTE(noop) only used for byte alignment; contains no data
                TracePacket::Overflow => events.push(EventType::Overflow),

                // NOTE(noop) RTIC tasks always execute in handler mode;
                // thread mode is always exited before a task is run and
                // returned to on WFI.
                TracePacket::ExceptionTrace {
                    exception,
                    action: _,
                } if exception == &VectActive::ThreadMode => (),

                TracePacket::ExceptionTrace { exception, action } => events.push(EventType::Task {
                    name: match self.maps.resolve_hardware_task(exception) {
                        Ok(Some(name)) => name,

                        // NOTE(noop) task dispatcher entered/exited: we
                        // have already (or will) forward a message
                        // about the software task itself.
                        Ok(None) => continue,

                        Err(e) => {
                            events.push(EventType::Unmappable(packet.clone(), e.to_string()));
                            continue;
                        }
                    },
                    action: match action {
                        ExceptionAction::Entered => TaskAction::Entered,
                        ExceptionAction::Exited => TaskAction::Exited,
                        ExceptionAction::Returned => TaskAction::Returned,
                    },
                }),
                TracePacket::DataTraceValue {
                    comparator,
                    access_type,
                    value,
                } if *access_type == MemoryAccessType::Write => {
                    events.push(match self.maps.resolve_software_task(comparator, value) {
                        Ok(Some(task_event)) => task_event,
                        Ok(None) => EventType::Unknown(packet.clone()), // not a software task DWT comparator
                        Err(e) => EventType::Unmappable(packet.clone(), e.to_string()),
                    });
                }
                _ => events.push(EventType::Unknown(packet.clone())),
            }
        }

        // map malformed packets
        events.append(
            &mut malformed_packets
                .iter()
                .map(|m| EventType::Invalid(m.to_owned()))
                .collect(),
        );

        EventChunk { timestamp, events }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Ensure an RTIC application can be properly parsed.
    #[test]
    fn parse_rtic_app() {
        let arguments = quote!(device = stm32f4::stm32f401);
        let ast = quote!(
            #[rtic::app(#arguments)]
            mod app {
                #[shared]
                struct Shared {}

                #[local]
                struct Local {}

                #[init]
                fn init(mut ctx: init::Context) -> (Shared, Local, init::Monotonics) {
                    (Shared {}, Local {}, init::Monotonics())
                }
            }
        );
        let src = quote!(
            #![no_std]
            #![no_main]

            use panic_halt as _;
            use rtic;

            #ast
        );

        TraceLookupMaps::parse_rtic_app(src).unwrap();
    }
}
