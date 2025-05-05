use {
    fx::{FxCtx, HttpRequest, HttpResponse, rpc},
    fx_utils::handle_http_axum_router,
    axum::{Router, routing::get},
};

#[rpc]
pub async fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    handle_http_axum_router(
        Router::new()
            .route("/api/mock/get", get(mock_http_get)),
        req
    ).await
}

async fn mock_http_get() -> &'static str {
    "hello fx!"
}
