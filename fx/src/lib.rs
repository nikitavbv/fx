mod sys;

pub use fx_core::{HttpRequest, HttpResponse};

pub fn read_http_request(addr: i64, len: i64) -> HttpRequest {
    let request = unsafe { std::slice::from_raw_parts(addr as *const u8, len as usize) };
    let (request, _): (HttpRequest, _) = bincode::decode_from_slice(&request, bincode::config::standard()).unwrap();
    request
}

pub fn send_http_response(response: HttpResponse) {
    let response = bincode::encode_to_vec(response, bincode::config::standard()).unwrap();
    unsafe { sys::send_http_response(response.as_ptr() as i64, response.len() as i64); }
}
