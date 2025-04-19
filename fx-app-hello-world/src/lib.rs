use fx_core::HttpResponse;

#[unsafe(no_mangle)]
pub extern "C" fn handle() -> i32 {
    let response = HttpResponse {
        body: "Hello World!".to_owned(),
    };
    // TODO: send response to host

    0
}
