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
        ::fx_sdk::inventory::submit! {
            ::fx_sdk::Handler::new(stringify!(#fn_name), || { ::fx_sdk::IntoHandler::into_boxed(#fn_name) })
        }
    };

    let output = quote! {
        #input_fn

        #ffi_fn
    };

    output.into()
}

#[proc_macro_attribute]
pub fn fetch(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(item as ItemFn);

    let fn_name = &input_fn.sig.ident;
    let ffi_fn = quote! {
        #[::fx_sdk::handler::ctor]
        fn __fx_register_handler_fetch() {
            ::fx_sdk::handler::register_http_fetch_handler(#fn_name);
        }
    };

    let output = quote! {
        #input_fn

        #ffi_fn
    };

    output.into()
}
