extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{self, parse_macro_input, ItemFn, LitInt, Stmt};

static mut TRACE_ID: usize = 0;

#[proc_macro_attribute]
pub fn trace(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut fun = parse_macro_input!(item as ItemFn);
    fun.block.stmts = {
        let id = syn::parse_str::<LitInt>(
            format!("{}", unsafe {
                let retval = TRACE_ID;
                TRACE_ID += 1;
                retval
            })
            .as_str(),
        )
        .unwrap();
        let trace_stmt = syn::parse2::<Stmt>(quote!(
            ::rtic_trace::set_current_task_id(#id);
        ))
        .unwrap();

        let mut stmts = vec![trace_stmt.clone()];
        stmts.append(&mut fun.block.stmts);
        stmts.push(trace_stmt);
        stmts
    };

    fun.into_token_stream().into()
}
