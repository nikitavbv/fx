use {
    fx::{HttpRequest, HttpResponse, rpc, utils::axum::handle_request},
    axum::{Router, routing::get},
};

#[rpc]
pub async fn http(req: HttpRequest) -> fx::Result<HttpResponse> {
    Ok(handle_request(
        Router::new()
            .route("/api/mock/get", get(mock_http_get)),
        req
    ).await)
}

async fn mock_http_get() -> &'static str {
    "hello fx!"
}
