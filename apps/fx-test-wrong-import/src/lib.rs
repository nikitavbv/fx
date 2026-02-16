use fx_sdk::{handler, HttpRequest, HttpResponse};

#[link(wasm_import_module = "fx")]
unsafe extern "C" {
    pub(crate) fn stream_poll_next(arg: i64);
}

#[handler::fetch]
pub async fn http(_req: HttpRequest) -> HttpResponse {
    unsafe { stream_poll_next(-1); }
    panic!("should not be called");
}
