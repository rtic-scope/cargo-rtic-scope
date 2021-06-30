use crate::build::CargoWrapper;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;

use anyhow::{bail, Context, Result};
use cargo_metadata::Artifact;
use chrono::Local;
use include_dir::include_dir;
use itm_decode::{ExceptionAction, TimestampedTracePackets, TracePacket};
use libloading;
use proc_macro2::{Ident, TokenStream, TokenTree};
use quote::{format_ident, quote};
use rtic_scope_api::{EventChunk, EventType, TaskAction};
use rtic_syntax;
use serde::{Deserialize, Serialize};
use syn;

type HwExceptionNumber = u8;
type SwExceptionNumber = usize;
type ExceptionIdent = String;
type TaskIdent = [String; 2];
type ExternalHwAssocs = BTreeMap<HwExceptionNumber, (TaskIdent, ExceptionIdent)>;
type InternalHwAssocs = BTreeMap<ExceptionIdent, TaskIdent>;
type SwAssocs = BTreeMap<SwExceptionNumber, Vec<String>>;

#[derive(Clone, Serialize, Deserialize)]
pub struct Metadata {
    maps: TaskResolveMaps,
    timestamp: chrono::DateTime<Local>,
    freq: u32,
}

impl Metadata {
    pub fn new(maps: TaskResolveMaps, timestamp: chrono::DateTime<Local>, freq: u32) -> Self {
        Self {
            maps,
            timestamp,
            freq,
        }
    }

    pub fn resolve_event_chunk(&self, packets: TimestampedTracePackets) -> Result<EventChunk> {
        let timestamp = || -> Result<chrono::DateTime<Local>> {
            // TODO handle all fields
            let itm_decode::Timestamp { base, delta, .. } = packets.timestamp;
            let seconds_since =
                (base.unwrap_or(0) + delta.context("delta missing")?) as f64 / self.freq as f64;
            let since = chrono::Duration::nanoseconds((seconds_since * 1e9).round() as i64);
            Ok(self.timestamp + since)
        }()
        .context("Failed to resolve absolute timestamp")?;

        let resolve_exception = |&excpt| -> Result<String> {
            use itm_decode::cortex_m::VectActive;

            match excpt {
                VectActive::ThreadMode => bail!("Don't know what to do with ThreadMode"), // XXX fix
                VectActive::Exception(e) => Ok(self
                    .maps
                    .exceptions
                    .get(&format!("{:?}", e))
                    .with_context(|| format!("Exception map is missing key {:?}", e))?
                    .join("::")),
                VectActive::Interrupt { irqn } => {
                    let (fun, _bind) = self
                        .maps
                        .interrupts
                        .get(&irqn)
                        .with_context(|| format!("Interrupt map is missing key {}", irqn))?;
                    Ok(fun.join("::"))
                }
            }
        };

        // convert itm_decode::TracePacket -> api::EventType
        let mut events = vec![];
        for packet in packets.packets.iter() {
            match packet {
                TracePacket::Overflow => {
                    events.push(EventType::Overflow);
                }
                TracePacket::ExceptionTrace { exception, action } => events.push(EventType::Task {
                    name: resolve_exception(exception)?,
                    action: match action {
                        ExceptionAction::Entered => TaskAction::Entered,
                        ExceptionAction::Exited => TaskAction::Exited,
                        ExceptionAction::Returned => TaskAction::Returned,
                    },
                }),
                _ => {
                    eprintln!("Don't know how to convert {:?}. Skipping...", packet);
                }
            }
        }

        Ok(EventChunk { timestamp, events })
    }
}

impl fmt::Display for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.maps)?;
        writeln!(f, "reset timestamp: {}", self.timestamp)?;
        writeln!(f, "trace clock frequency: {} Hz", self.freq)?;

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TaskResolveMaps {
    pub exceptions: InternalHwAssocs,
    pub interrupts: ExternalHwAssocs,
    pub sw_assocs: SwAssocs,
}

impl fmt::Display for TaskResolveMaps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Here C++ reigns superior with its generic lambdas.
        macro_rules! display_map {
            ($h:expr, $m:expr) => {{
                writeln!(f, "{}:", $h)?;
                for (k, v) in $m.iter() {
                    writeln!(f, "\t {} -> {:?}", k, v)?;
                }

                Ok(())
            }};
        }

        display_map!("exceptions", self.exceptions)?;
        display_map!("interrupts", self.interrupts)?;
        display_map!("software tasks", self.sw_assocs)
    }
}

pub struct TaskResolver<'a> {
    cargo: &'a CargoWrapper,
    app: TokenStream,
    app_args: TokenStream,
}

impl<'a> TaskResolver<'a> {
    pub fn new(artifact: &Artifact, cargo: &'a CargoWrapper) -> Result<Self> {
        // parse the RTIC app from the source file
        let src = fs::read_to_string(&artifact.target.src_path).context("Failed to open file")?;
        let mut rtic_app = syn::parse_str::<TokenStream>(&src)
            .context("Failed to tokenize file")?
            .into_iter()
            .skip_while(|token| {
                // TODO improve this
                if let TokenTree::Group(g) = token {
                    return g.stream().into_iter().nth(0).unwrap().to_string().as_str() != "app";
                }
                true
            });
        let app_args = {
            let mut args: Option<TokenStream> = None;
            if let TokenTree::Group(g) = rtic_app.next().unwrap() {
                // TODO improve this
                if let TokenTree::Group(g) = g.stream().into_iter().nth(1).unwrap() {
                    args = Some(g.stream());
                }
            }
            args.unwrap()
        };
        let app = rtic_app.collect::<TokenStream>();

        Ok(TaskResolver {
            cargo,
            app,
            app_args,
        })
    }

    pub fn resolve(&self) -> Result<TaskResolveMaps> {
        let (exceptions, interrupts) = self.hardware_tasks()?;
        let sw_assocs = self.software_tasks()?;

        Ok(TaskResolveMaps {
            exceptions,
            interrupts,
            sw_assocs,
        })
    }

    /// Parses an RTIC `mod app { ... }` declaration and associates the full
    /// path of the functions that are decorated with the `#[trace]`-macro
    /// with it's assigned task ID.
    fn software_tasks(&self) -> Result<SwAssocs> {
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

        let app = syn::parse2::<syn::Item>(self.app.clone())?;
        let mut ctx: Vec<syn::Ident> = vec![];
        let mut assocs = SwAssocs::new();
        let mut id_gen = TaskIDGenerator::new();

        fn traverse_item(
            item: &syn::Item,
            ctx: &mut Vec<syn::Ident>,
            assocs: &mut SwAssocs,
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
                            traverse_item(&item, ctx, assocs, id_gen);
                        }
                    }
                    ctx.pop();
                }
                _ => (),
            }
        }

        traverse_item(&app, &mut ctx, &mut assocs, &mut id_gen);

        Ok(assocs)
    }

    /// Parses an RTIC `#[app(device = ...)] mod app { ... }` declaration
    /// and associates the full path of hardware task functions to their
    /// exception numbers as reported by the target.
    fn hardware_tasks(&self) -> Result<(InternalHwAssocs, ExternalHwAssocs)> {
        let mut settings = rtic_syntax::Settings::default();
        settings.parse_binds = true;
        let (app, _analysis) =
            rtic_syntax::parse2(self.app_args.clone(), self.app.clone(), settings)?;

        // Find the bound exceptions from the #[task(bound = ...)]
        // arguments. Further, partition internal and external interrupts.
        //
        // For external exceptions (those defined in PAC::Interrupt), we
        // need to resolve the number we receive over ITM back to the
        // interrupt name. For internal interrupts, the name of the
        // execption is received over ITM.
        let (int_binds, ext_binds): (Vec<Ident>, Vec<Ident>) = app
            .hardware_tasks
            .iter()
            .map(|(_name, hwt)| hwt.args.binds.clone())
            .partition(|bind| {
                [
                    "Reset",
                    "NMI",
                    "HardFault",
                    "MemManage",
                    "BusFault",
                    "UsageFault",
                    "SVCall",
                    "DebugMonitor",
                    "PendSV",
                    "SysTick",
                ]
                .iter()
                .find(|&&int| int == bind.to_string())
                .is_some()
            });
        let binds = ext_binds.clone();

        // Parse out the PAC from #[app(device = ...)] and resolve exception
        // numbers from bound idents.
        let device_arg: Vec<syn::Ident> = match app.args.device.as_ref() {
            None => bail!("expected argument #[app(device = ...)] is missing"),
            Some(device) => device.segments.iter().map(|ps| ps.ident.clone()).collect(),
        };
        let excpt_nrs = match &device_arg[..] {
            _ if ext_binds.is_empty() => BTreeMap::<Ident, u8>::new(),
            [crate_name] => self.resolve_int_nrs(&binds, &crate_name, None)?,
            [crate_name, crate_feature] => {
                self.resolve_int_nrs(&binds, &crate_name, Some(&crate_feature))?
            }
            _ => bail!("argument passed to #[app(device = ...)] cannot be parsed"),
        };

        let int_assocs: InternalHwAssocs = app
            .hardware_tasks
            .iter()
            .filter_map(|(name, hwt)| {
                let bind = &hwt.args.binds;
                if let Some(_) = int_binds.iter().find(|&b| b == bind) {
                    Some((bind.to_string(), ["app".to_string(), name.to_string()]))
                } else {
                    None
                }
            })
            .collect();

        let ext_assocs: ExternalHwAssocs = app
            .hardware_tasks
            .iter()
            .filter_map(|(name, hwt)| {
                let bind = &hwt.args.binds;
                if let Some(int) = excpt_nrs.get(&bind) {
                    Some((
                        int.clone(),
                        (["app".to_string(), name.to_string()], bind.to_string()),
                    ))
                } else {
                    None
                }
            })
            .collect();

        Ok((int_assocs, ext_assocs))
    }

    fn resolve_int_nrs(
        &self,
        binds: &[Ident],
        crate_name: &Ident,
        crate_feature: Option<&Ident>,
    ) -> Result<BTreeMap<Ident, u8>> {
        const ADHOC_FUNC_PREFIX: &str = "rtic_scope_func_";

        // Extract adhoc source to a temporary directory and apply adhoc
        // modifications.
        let target_dir = self
            .cargo
            .target_dir()
            .unwrap()
            .join("cargo-rtic-trace-libadhoc");
        include_dir!("assets/libadhoc")
            .extract(&target_dir)
            .context("Failed to extract libadhoc")?;
        // Add required crate (and optional feature) as dependency
        {
            let mut manifest = fs::OpenOptions::new()
                .append(true)
                .open(target_dir.join("Cargo.toml"))?;
            let dep = format!(
                "\n{} = {{ version = \"\", features = [\"{}\"]}}\n",
                crate_name,
                match crate_feature {
                    Some(feat) => format!("{}", feat),
                    None => "".to_string(),
                }
            );
            manifest.write_all(dep.as_bytes())?;
        }
        // Prepare lib.rs
        {
            // Import PAC::Interrupt
            let mut src = fs::OpenOptions::new()
                .append(true)
                .open(target_dir.join("src/lib.rs"))?;
            let import = match crate_feature {
                Some(_) => quote!(use #crate_name::#crate_feature::Interrupt;),
                None => quote!(use #crate_name::Interrupt;),
            };
            src.write_all(format!("\n{}\n", import).as_bytes())?;

            // Generate the functions that must be exported
            for bind in binds {
                let fun = format_ident!("{}{}", ADHOC_FUNC_PREFIX, bind);
                let int_ident = format_ident!("{}", bind);
                let fun = quote!(
                    #[no_mangle]
                    pub extern fn #fun() -> u8 {
                        Interrupt::#int_ident.nr()
                    }
                );
                src.write_all(format!("\n{}\n", fun).as_bytes())?;
            }
        }

        // Build the adhoc library, load it, and resolve all exception idents
        let artifact = self.cargo.build(&target_dir, "".to_string(), "cdylib")?;
        let lib = unsafe { libloading::Library::new(artifact.filenames.first().unwrap())? };
        Ok(binds
            .into_iter()
            .map(|b| {
                let func: libloading::Symbol<extern "C" fn() -> u8> = unsafe {
                    lib.get(format!("{}{}", ADHOC_FUNC_PREFIX, b).as_bytes())
                        .unwrap()
                };
                (b.clone(), func())
            })
            .collect())
    }
}
