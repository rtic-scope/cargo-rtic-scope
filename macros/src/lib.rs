extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use std::{env, fs, io::Write, path::Path};
use syn::{self, parse_macro_input, Ident, ItemFn, LitInt, Stmt};

const SW_TRACE_RESOLVE_DIR_ENV: &str = "RTIC_TRACE_RESOLVE_DIR";

static mut TRACE_ID: usize = 0;

#[proc_macro_attribute]
pub fn trace(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut fun = parse_macro_input!(item as ItemFn);

    // Generate a unique (software) task ID by strictly increasing a
    // variable that preserves state over multiple macro calls.
    let task_id = syn::parse_str::<LitInt>(
        format!("{}", unsafe {
            let id = TRACE_ID;
            TRACE_ID += 1;
            id
        })
        .as_str(),
    )
    .unwrap();

    // Write task id and function name to file for the host-side trace
    // daemon to consume.
    write_assoc_to_file(task_id.clone(), fun.sig.ident.clone());

    fun.block.stmts = {
        // Insert a statement at the start and end of the given function
        // that writes the unique task ID to the watchpoint address.
        let trace_stmt = syn::parse2::<Stmt>(quote!(
            ::rtic_trace::__write_trace_payload(#task_id);
        ))
        .unwrap();
        let mut stmts = vec![trace_stmt.clone()];
        stmts.append(&mut fun.block.stmts);
        stmts.push(trace_stmt);
        stmts
    };

    fun.into_token_stream().into()
}

fn write_assoc_to_file(id: LitInt, fun: Ident) {
    let resolve_dir = env::var(SW_TRACE_RESOLVE_DIR_ENV).expect(
        format!(
            "Expected environmental variable {} is not set",
            SW_TRACE_RESOLVE_DIR_ENV
        )
        .as_str(),
    );
    let resolve_dir = Path::new(&resolve_dir);
    fs::create_dir_all(resolve_dir).expect(
        format!(
            "Unable to create {} for task ID association",
            resolve_dir.to_str().unwrap()
        )
        .as_str(),
    );
    let resolve_file = resolve_dir.join("software-tasks");
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(resolve_file.clone())
        .expect(
            format!(
                "Failed to open association file {} for append",
                resolve_file.to_str().unwrap()
            )
            .as_str(),
        );
    f.write_all(format!("({}, {})\n", id, fun).as_bytes())
        .expect(
            format!(
                "Failed to append to association file {}",
                resolve_file.to_str().unwrap()
            )
            .as_str(),
        );
}
