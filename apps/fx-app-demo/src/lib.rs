use {
    tracing::info,
    fx_sdk::{HttpRequestV2, HttpResponse, handler},
};

#[handler::fetch]
pub async fn http(_req: HttpRequestV2) -> HttpResponse {
    info!("hello from wasm service!");
    HttpResponse::new().with_body("hello fx!\n")
}
