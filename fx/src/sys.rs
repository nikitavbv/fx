// exports:
#[unsafe(no_mangle)]
pub extern "C" fn _fx_malloc(size: i64) -> i64 {
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size as usize, 1).unwrap()) as i64 }
}

// imports:
#[link(wasm_import_module = "fx")]
unsafe extern "C" {
    pub(crate) fn send_http_response(ptr: i64, len: i64);
}
