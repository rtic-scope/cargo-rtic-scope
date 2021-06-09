use anyhow::{bail, Result};
use cargo;
use include_dir::include_dir;
use libloading;
use proc_macro2::Ident;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use rtic_syntax;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tempfile;

type HwExceptionNumber = u8;
type SwExceptionNumber = usize;
type ExceptionIdent = syn::Ident;
type TaskIdent = [syn::Ident; 2];
type ExternalHwAssocs = BTreeMap<HwExceptionNumber, (TaskIdent, ExceptionIdent)>;
type InternalHwAssocs = BTreeMap<ExceptionIdent, TaskIdent>;
type SwAssocs = BTreeMap<SwExceptionNumber, Vec<syn::Ident>>;

/// Parses an RTIC `#[app(device = ...)] mod app { ... }` declaration
/// and associates the full path of hardware task functions to their
/// exception numbers as reported by the target.
pub fn hardware_tasks(
    app: TokenStream,
    args: TokenStream,
) -> Result<(InternalHwAssocs, ExternalHwAssocs)> {
    let mut settings = rtic_syntax::Settings::default();
    settings.parse_binds = true;
    let (app, _analysis) = rtic_syntax::parse2(args, app.clone(), settings)?;

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
        [crate_name] => resolve_int_nrs(&binds, &crate_name, None)?,
        [crate_name, crate_feature] => resolve_int_nrs(&binds, &crate_name, Some(&crate_feature))?,
        _ => bail!("argument passed to #[app(device = ...)] cannot be parsed"),
    };

    let int_assocs: InternalHwAssocs = app
        .hardware_tasks
        .iter()
        .filter_map(|(name, hwt)| {
            let bind = &hwt.args.binds;
            if let Some(_) = int_binds.iter().find(|&b| b == bind) {
                Some((bind.clone(), [syn::parse_quote!(app), name.clone()]))
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
                    ([syn::parse_quote!(app), name.clone()], bind.clone()),
                ))
            } else {
                None
            }
        })
        .collect();

    Ok((int_assocs, ext_assocs))
}

fn resolve_int_nrs(
    binds: &[Ident],
    crate_name: &Ident,
    crate_feature: Option<&Ident>,
) -> Result<BTreeMap<Ident, u8>> {
    const ADHOC_FUNC_PREFIX: &str = "rtic_scope_func_";
    const ADHOC_TARGET_DIR_ENV: &str = "RTIC_SCOPE_CARGO_TARGET_DIR";

    // Prepare a temporary directory for adhoc build
    let tmpdir = tempfile::tempdir()?;
    include_dir!("assets/libadhoc").extract(tmpdir.path())?;
    // Add required crate (and eventual feature) as dependency
    {
        let mut manifest = fs::OpenOptions::new()
            .append(true)
            .open(tmpdir.path().join("Cargo.toml"))?;
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
    {
        // Import PAC::Interrupt
        let mut src = fs::OpenOptions::new()
            .append(true)
            .open(tmpdir.path().join("src/lib.rs"))?;
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

    // NOTE: change working directory so that our build environment does
    // not contain any eventual `.cargo/config`.
    assert!(env::set_current_dir(tmpdir.path()).is_ok());
    let cc = cargo::util::config::Config::default()?;
    let mut ws = cargo::core::Workspace::new(&tmpdir.path().join("Cargo.toml"), &cc)?;
    let target_dir = if let Ok(target) =
        env::var("CARGO_TARGET_DIR").or_else(|_| env::var(ADHOC_TARGET_DIR_ENV))
    {
        PathBuf::from(target)
    } else {
        tmpdir.path().join("target/")
    };
    ws.set_target_dir(cargo::util::Filesystem::new(target_dir));
    let build = cargo::ops::compile(
        &ws,
        &cargo::ops::CompileOptions::new(&cc, cargo::core::compiler::CompileMode::Build)?,
    )?;
    assert!(build.cdylibs.len() == 1);
    let lib = unsafe { libloading::Library::new(build.cdylibs.first().unwrap().path.as_os_str())? };
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

/// Parses an RTIC `mod app { ... }` declaration and associates the full
/// path of the functions that are decorated with the `#[trace]`-macro
/// with it's assigned task ID.
pub fn software_tasks(app: TokenStream) -> Result<SwAssocs> {
    let app = syn::parse2::<syn::Item>(app)?;
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
                    assocs.insert(id_gen.generate(), ctx.clone());
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
