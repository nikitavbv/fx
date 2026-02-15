use {
    fx_sdk::{self as fx, HttpRequest, HttpResponse, handler, utils::axum::handle_request},
    axum::{Router, routing::get},
};

#[handler::fetch]
pub async fn http(req: HttpRequest) -> HttpResponse {
    handle_request(
        Router::new()
            .route("/api/mock/get", get(mock_http_get)),
        req
    ).await
}

async fn mock_http_get() -> &'static str {
    "hello fx!"
}
