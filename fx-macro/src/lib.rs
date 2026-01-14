use {
    proc_macro::TokenStream,
    quote::quote,
    syn::{parse_macro_input, ItemFn},
};

#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let ffi_fn = quote! {
        ::fx::inventory::submit! {
            ::fx::Handler::new(stringify!(#fn_name), || { ::fx::IntoHandler::into_boxed(#fn_name) })
        }
    };

    let output = quote! {
        #input_fn

        #ffi_fn
    };

    output.into()
}
