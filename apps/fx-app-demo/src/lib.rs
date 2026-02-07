use {
    fx_sdk::{HttpRequestV2, HttpResponse, handler},
    tracing::info,
};

#[handler::fetch]
pub async fn http(req: HttpRequestV2) -> HttpResponse {
    info!("hello from wasm service!");

    if req.uri().path().starts_with("/test/status-code") {
        HttpResponse::new().with_body("this returns custom status code.\n")
    } else {
        HttpResponse::new().with_body("hello fx!\n")
    }
}
