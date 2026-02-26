use {
    fx_sdk::{HttpRequest, HttpResponse, utils::axum::handle_request, handler},
    axum::{Router, routing::get},
    tracing::info,
};

#[handler::fetch]
pub async fn http(req: HttpRequest) -> HttpResponse {
    info!("handling request in rpc target function");

    handle_request(
        Router::new()
            .route("/", get(index)),
        req,
    ).await
}

async fn index() -> &'static str {
    "hello from other function!"
}
