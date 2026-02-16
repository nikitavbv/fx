#[unsafe(no_mangle)]
fn __fx_handler_http(request_resource: u64) -> u64 {
    panic!("should not be called");
}
