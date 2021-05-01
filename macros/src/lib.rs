extern crate proc_macro;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::{self, ItemFn, Stmt};

#[proc_macro_attribute]
pub fn trace(attrs: TokenStream, item: TokenStream) -> TokenStream {
    let _attrs: TokenStream2 = attrs.into();
    let item: TokenStream2 = item.into();

    let mut fun = syn::parse2::<ItemFn>(item).unwrap();
    fun.block.stmts = {
        let trace_stmt = syn::parse2::<Stmt>(quote!(
            ::rtic_trace::set_current_task_id(42);
        ))
            .unwrap();

        let mut stmts = vec![trace_stmt.clone()];
        stmts.append(&mut fun.block.stmts);
        stmts.push(trace_stmt);
        stmts
    };

    fun.into_token_stream().into()
}
