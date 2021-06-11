use crate::building;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use cargo_metadata::Artifact;
use include_dir::include_dir;
use libloading;
use proc_macro2::{Ident, TokenStream, TokenTree};
use quote::{format_ident, quote};
use rtic_syntax;
use syn;
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
fn hardware_tasks(
    app: TokenStream,
    args: TokenStream,
    target_dir: PathBuf,
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
        [crate_name] => resolve_int_nrs(&binds, &crate_name, None, target_dir.as_path())?,
        [crate_name, crate_feature] => resolve_int_nrs(
            &binds,
            &crate_name,
            Some(&crate_feature),
            target_dir.as_path(),
        )?,
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
    target_dir: &Path,
) -> Result<BTreeMap<Ident, u8>> {
    const ADHOC_FUNC_PREFIX: &str = "rtic_scope_func_";

    // Extract adhoc source to a temporary directory and apply adhoc
    // modifications.
    let tmpdir = tempfile::tempdir()?;
    include_dir!("assets/libadhoc").extract(tmpdir.path())?;
    // Add required crate (and optional feature) as dependency
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
    // Prepare lib.rs
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
    let artifact = building::cargo_build(
        tmpdir.path(),
        &["--target-dir", target_dir.to_str().unwrap()],
        "cdylib",
    )?;
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
fn software_tasks(app: TokenStream) -> Result<SwAssocs> {
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

pub fn resolve_tasks(
    artifact: &Artifact,
) -> Result<((ExternalHwAssocs, InternalHwAssocs), SwAssocs)> {
    // parse the RTIC app from the source file
    let src = fs::read_to_string(&artifact.target.src_path)
        .context("Failed to open RTIC app source file")?;
    let mut rtic_app = syn::parse_str::<TokenStream>(&src)
        .context("Failed to parse RTIC app source file")?
        .into_iter()
        .skip_while(|token| {
            // TODO improve this
            if let TokenTree::Group(g) = token {
                return g.stream().into_iter().nth(0).unwrap().to_string().as_str() != "app";
            }
            true
        });
    let args = {
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

    // Find a suitable target directory from --bin which we'll reuse
    // for building the adhoc library, unless CARGO_TARGET_DIR is
    // set.
    let target_dir = if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
        PathBuf::from(target_dir)
    } else {
        // Adhoc will end up under some target/thumbv7em-.../
        // which is technically incorrect, but scanning for a
        // "target/" in the path is unstable if CARGO_TARGET_DIR is
        // set, which may not contain a "target/". Our reuse of the
        // directory is nevertheless commented with a verbose
        // directory name.
        let mut path = artifact.executable.clone().unwrap();
        path.pop();
        path.push("rtic-trace-adhoc-target");
        // NOTE(_all): we do not necessarily need to create all
        // directories, but we do not want to fail if the directory
        // exists.
        fs::create_dir_all(&path).unwrap();
        path
    };

    let (ints, excps) = hardware_tasks(app.clone(), args, target_dir)?;
    let sw_tasks = software_tasks(app)?;

    Ok(((excps, ints), sw_tasks))
}
