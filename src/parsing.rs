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

type HwAssocs = BTreeMap<u8, (syn::Ident, syn::Ident)>;
type SwAssocs = BTreeMap<usize, Vec<syn::Ident>>;

pub fn hardware_tasks(app: TokenStream, args: TokenStream) -> Result<HwAssocs, ()> {
    let mut settings = rtic_syntax::Settings::default();
    settings.parse_binds = true;
    let (rtic_app, _analysis) = rtic_syntax::parse2(args, app.clone(), settings).unwrap();

    // Associate hardware tasks to their interrupt numbers
    let (crate_name, crate_feature) = {
        let mut segs: Vec<Ident> = rtic_app
            .args
            .device
            .as_ref()
            .unwrap()
            .segments
            .iter()
            .map(|ps| ps.ident.clone())
            .collect();
        (segs.remove(0), segs.remove(0))
    };
    let binds: Vec<Ident> = rtic_app
        .hardware_tasks
        .iter()
        .map(|(_name, ht)| ht.args.binds.clone())
        .collect();
    let int_nrs = resolve_int_nrs(&binds, &crate_name, &crate_feature);
    Ok(rtic_app
        .hardware_tasks
        .iter()
        .map(|(name, ht)| {
            let bind = &ht.args.binds;
            let int = int_nrs.get(&bind).unwrap();
            (int.clone(), (name.clone(), bind.clone()))
        })
        .collect())
}

const ADHOC_FUNC_PREFIX: &str = "rtic_scope_func_";
const ADHOC_TARGET_DIR_ENV: &str = "RTIC_SCOPE_CARGO_TARGET_DIR";

pub fn resolve_int_nrs(
    binds: &[Ident],
    crate_name: &Ident,
    crate_feature: &Ident,
) -> BTreeMap<Ident, u8> {
    // generate a temporary directory
    let tmpdir = tempfile::tempdir().unwrap();

    // extract the skeleton crate
    include_dir!("assets/libadhoc")
        .extract(tmpdir.path())
        .unwrap();

    // append the crate (and its feature) we need
    {
        let mut lib_manifest = fs::OpenOptions::new()
            .append(true)
            .open(tmpdir.path().join("Cargo.toml"))
            .unwrap();
        lib_manifest
            .write_all(
                format!(
                    "\n{} = {{ version = \"\", features = [\"{}\"]}}\n",
                    crate_name, crate_feature
                )
                .as_bytes(),
            )
            .unwrap();
    }

    // append the includes and functions we need
    let mut lib_src = fs::OpenOptions::new()
        .append(true)
        .open(tmpdir.path().join("src/lib.rs"))
        .unwrap();
    let include = quote!(
        use #crate_name::#crate_feature::Interrupt;
    );
    lib_src
        .write_all(format!("\n{}\n", include).as_bytes())
        .unwrap();
    for bind in binds {
        let func = format_ident!("{}{}", ADHOC_FUNC_PREFIX, bind);
        let int_field = format_ident!("{}", bind);
        let src = quote!(
            #[no_mangle]
            pub extern fn #func() -> u8 {
                Interrupt::#int_field.nr()
            }
        );
        lib_src
            .write_all(format!("\n{}\n", src).as_bytes())
            .unwrap();
    }

    // cargo build the adhoc cdylib library
    let cc = cargo::util::config::Config::default().unwrap();
    let mut ws = cargo::core::Workspace::new(&tmpdir.path().join("Cargo.toml"), &cc).unwrap();
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
        &cargo::ops::CompileOptions::new(&cc, cargo::core::compiler::CompileMode::Build).unwrap(),
    )
    .unwrap();
    assert!(build.cdylibs.len() == 1);

    // Load the library and find the bind mappings
    let lib = unsafe {
        libloading::Library::new(build.cdylibs.first().unwrap().path.as_os_str()).unwrap()
    };
    binds
        .into_iter()
        .map(|b| {
            let func: libloading::Symbol<extern "C" fn() -> u8> = unsafe {
                lib.get(format!("{}{}", ADHOC_FUNC_PREFIX, b).as_bytes())
                    .unwrap()
            };
            (b.clone(), func())
        })
        .collect()
}

/// Parses an RTIC app and associates the absolute path of the function
/// that is decorated with [trace], with it's assigned task ID, received
/// over SWO.
pub fn software_tasks(app: TokenStream) -> Result<SwAssocs, syn::Error> {
    // iterate down along scopes (mod, fn), and keep tab on current context
    // when we find a #[trace], genrate the task id, and assign the absolute function path to it.

    let app = syn::parse2::<syn::ItemMod>(app)?;
    let mut ctx: Vec<syn::Ident> = vec![app.ident.clone()];
    let mut assocs = SwAssocs::new();
    let mut task_id: usize = 0;

    fn traverse_item(
        item: &syn::Item,
        ctx: &mut Vec<syn::Ident>,
        assocs: &mut SwAssocs,
        task_id: &mut usize,
    ) {
        match item {
            syn::Item::Fn(fun) => {
                ctx.push(fun.sig.ident.clone());
                let trace_attr = syn::parse2::<syn::Path>(quote!(trace)).unwrap();
                if fun.attrs.iter().any(|a| a.path == trace_attr) {
                    assocs.insert(*task_id, ctx.clone());
                    *task_id += 1;
                }

                for item in fun.block.stmts.iter().filter_map(|stmt| match stmt {
                    syn::Stmt::Item(item) => Some(item),
                    _ => None,
                }) {
                    traverse_item(item, ctx, assocs, task_id);
                }
                ctx.pop();
            }
            _ => (),
        }
    }

    let (_, items) = app.content.unwrap();
    for item in items {
        traverse_item(&item, &mut ctx, &mut assocs, &mut task_id);
    }

    Ok(assocs)
}
