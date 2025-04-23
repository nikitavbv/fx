use {
    fx::{FxCtx, HttpRequest, HttpResponse, rpc},
    axum::{Router, routing::get},
    fx_utils::handle_http_axum_router,
};

#[rpc]
pub fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    let app = Router::new()
        .route("/", get(home))
        .route("/something", get(something));

    handle_http_axum_router(app, req)
}

async fn home() -> &'static str {
    "hello from dashboard home! v2"
}

async fn something() -> &'static str {
    "something"
}
