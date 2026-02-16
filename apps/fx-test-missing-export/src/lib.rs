#[unsafe(no_mangle)]
fn __fx_handler_http(_: u64) -> u64 {
    panic!("should not be called");
}
