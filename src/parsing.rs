use quote::quote;
use std::collections::BTreeMap;

type Assocs = BTreeMap<usize, Vec<syn::Ident>>;

/// Parses an RTIC app and associates the absolute path of the function
/// that is decorated with [trace], with it's assigned task ID, received
/// over SWO.
pub fn software_tasks(app: proc_macro2::TokenStream) -> Result<Assocs, syn::Error> {
    // iterate down along scopes (mod, fn), and keep tab on current context
    // when we find a #[trace], genrate the task id, and assign the absolute function path to it.

    let app = syn::parse2::<syn::ItemMod>(app)?;
    let mut ctx: Vec<syn::Ident> = vec![app.ident.clone()];
    let mut assocs = Assocs::new();
    let mut task_id: usize = 0;

    fn traverse_item(
        item: &syn::Item,
        ctx: &mut Vec<syn::Ident>,
        assocs: &mut Assocs,
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
