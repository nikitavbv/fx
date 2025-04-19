extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the input function
    let input_fn = parse_macro_input!(item as ItemFn);

    // Extract function details
    let fn_name = &input_fn.sig.ident; // e.g., `handle`
    let fn_vis = &input_fn.vis; // e.g., `pub`
    let fn_attrs = &input_fn.attrs; // Preserve any additional attributes
    let fn_block = &input_fn.block; // Function body
    let fn_inputs = &input_fn.sig.inputs; // Function parameters (e.g., `req: HttpRequest`)
    let fn_output = &input_fn.sig.output; // Return type (e.g., `-> HttpResponse`)

    // Generate the FFI wrapper function `_fx_handle`
    let ffi_fn = quote! {
        #[unsafe(no_mangle)]
        pub extern "C" fn _fx_handle(addr: i64, len: i64) -> i64 {
            fx::send_http_response(#fn_name(fx::read_http_request(addr, len)));
            0
        }
    };

    // Combine the original function and the generated FFI function
    let output = quote! {
        #ffi_fn

        #(#fn_attrs)*
        #fn_vis fn #fn_name(#fn_inputs) #fn_output #fn_block
    };

    output.into()
}
