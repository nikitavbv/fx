use fx_core::HttpResponse;

#[unsafe(no_mangle)]
pub extern "C" fn handle() -> i32 {
    let response = HttpResponse {
        body: "Hello World!".to_owned(),
    };
    let response = bincode::encode_to_vec(response, bincode::config::standard()).unwrap();
    unsafe { send_http_response(response.as_ptr() as i64, response.len() as i64); }

    0
}

#[link(wasm_import_module = "fx")]
unsafe extern "C" {
fn send_http_response(ptr: i64, len: i64);
}
