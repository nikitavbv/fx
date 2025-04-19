use fx::{send_http_response, read_http_request, HttpRequest, HttpResponse};

#[unsafe(no_mangle)]
pub extern "C" fn handle(addr: i64, len: i64) -> i64 {
    send_http_response(HttpResponse {
        body: format!("Hello from {:?}", read_http_request(addr, len).url),
    });
    0
}
