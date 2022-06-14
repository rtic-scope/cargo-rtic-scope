extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{self, parse_macro_input, ItemFn, LitInt, Stmt};

static mut TRACE_ID: usize = 0;

#[proc_macro_attribute]
pub fn trace(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    let mut fun = parse_macro_input!(item as ItemFn);
    fun.block.stmts = {
        // Generate a unique (software) task ID by strictly increasing a
        // variable that preserves state over multiple macro calls.
        let task_id = syn::parse_str::<LitInt>(
            format!("{}", unsafe {
                let id = TRACE_ID;
                TRACE_ID += 1;
                if TRACE_ID > u8::MAX.into() {
                    panic!("255 software tasks are supported at maximum");
                }
                id
            })
            .as_str(),
        )
        .unwrap();

        // Wrap the task body in a closure, write the enter UTID, call
        // the closure and save the return value, write the exit UTID,
        // and lastly return the value returned by the closure.
        let prologue = syn::parse2::<Stmt>(quote!(
            ::cortex_m_rtic_trace::__write_enter_id(#task_id);
        ))
        .unwrap();
        let epilogue = syn::parse2::<Stmt>(quote!(
            ::cortex_m_rtic_trace::__write_exit_id(#task_id);
        ))
        .unwrap();
        let call = syn::parse2::<Stmt>(quote!(
            let retval = closure();
        ))
        .unwrap();
        let ret = syn::parse2::<Stmt>(quote!(
            return retval;
        ))
        .unwrap();
        let closure = {
            let block_stmts = fun.block.stmts;
            // closure must be mut for task Context to be &mut.
            syn::parse2::<Stmt>(quote!(
                let mut closure = || {
                    #(#block_stmts)*
                };
            ))
            .unwrap()
        };

        vec![closure, prologue, call, epilogue, ret]
    };

    fun.into_token_stream().into()
}
