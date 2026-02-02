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
        #[unsafe(no_mangle)]
        fn __fx_handler_http(request_resource: u64) -> u64 {
            ::fx_sdk::logging::set_panic_hook();
            ::fx_sdk::logging::init_logger();

            let request_resource = ::fx_sdk::sys::ResourceId::new(request_resource);
            let request = ::fx_sdk::HttpRequestV2::from_host_resource(request_resource);

            let response_future = #fn_name(request);

            let future = ::fx_sdk::sys::wrap_function_response_future(response_future);

            future.as_u64()
        }
    };

    let output = quote! {
        #input_fn

        #ffi_fn
    };

    output.into()
}
