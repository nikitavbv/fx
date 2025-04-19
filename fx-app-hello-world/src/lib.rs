use fx_core::{HttpResponse, HttpRequest};

#[unsafe(no_mangle)]
pub extern "C" fn handle(addr: i64, len: i64) -> i64 {
    let request = unsafe { std::slice::from_raw_parts(addr as *const u8, len as usize) };
    let (request, _): (HttpRequest, _) = bincode::decode_from_slice(&request, bincode::config::standard()).unwrap();

    let response = HttpResponse {
        body: format!("Hello from {:?}", request.url),
    };
    let response = bincode::encode_to_vec(response, bincode::config::standard()).unwrap();
    unsafe { send_http_response(response.as_ptr() as i64, response.len() as i64); }

    0
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: i64) -> i64 {
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size as usize, 1).unwrap()) as i64 }
}

#[link(wasm_import_module = "fx")]
unsafe extern "C" {
fn send_http_response(ptr: i64, len: i64);
}
