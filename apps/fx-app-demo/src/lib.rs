use {
    fx_sdk::{HttpRequestV2, HttpResponse, handler, StatusCode},
    tracing::info,
};

#[handler::fetch]
pub async fn http(req: HttpRequestV2) -> HttpResponse {
    info!("hello from wasm service!");
    HttpResponse::new().with_body("hello fx!\n")
}
