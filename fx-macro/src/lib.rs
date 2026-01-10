use {
    proc_macro::TokenStream,
    quote::quote,
    syn::{parse_macro_input, ItemFn},
};

#[proc_macro_attribute]
pub fn rpc(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let fn_vis = &input_fn.vis;
    let fn_attrs = &input_fn.attrs;
    let fn_block = &input_fn.block;
    let fn_inputs = &input_fn.sig.inputs;
    let fn_output = &input_fn.sig.output;

    let wrapper_name = syn::Ident::new(&format!("_fx_rpc_{}", fn_name), proc_macro2::Span::call_site());

    let ffi_fn = quote! {
        fx::inventory::submit! {
            fx::Handler::new("#handler_name", || { fx::IntoHandler::into_boxed(#fn_name) })
        }
    };

    let output = quote! {
        #ffi_fn

        #(#fn_attrs)*
        #fn_vis async fn #fn_name(#fn_inputs) #fn_output #fn_block
    };

    output.into()
}
