use {
    axum::{Router, routing::get},
    fx_sdk::{handler, HttpRequest, HttpResponse, utils::axum::handle_request},
};


#[handler::fetch]
pub async fn http(req: HttpRequest) -> HttpResponse {
    handle_request(
        Router::new().route("/", get(home)),
        req
    ).await
}

async fn home() -> &'static str {
    "hello from function to remove!"
}
