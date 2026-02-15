use {
    tracing::info,
    fx_sdk::{HttpRequest, HttpResponse, handler},
};

#[handler::fetch]
pub async fn http(_req: HttpRequest) -> HttpResponse {
    info!("hello from wasm service!");
    HttpResponse::new().with_body("hello fx!\n")
}
