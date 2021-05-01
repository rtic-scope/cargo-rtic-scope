extern crate proc_macro;
use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn trace(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    // rtic_sw_task_trace::trace(attrs, item)
    item


    // let args: TokenStream2 = args.into();
    // let input: TokenStream2 = input.into();

    // TODO copy compile error formatting from rtic-syntax to print what we get here.
    // We need to check if we can chain proc macros
}
