use {
    fx::{FxCtx, HttpRequest, HttpResponse, rpc},
    axum::{Router, routing::get, response::{Response, IntoResponse}},
    leptos::prelude::*,
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

async fn home() -> impl IntoResponse {
    render_component(view! {
        Welcome <b>home</b>!
    })
}

async fn something() -> impl IntoResponse {
    "something"
}

fn render_component(component: impl IntoView + 'static) -> Response {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(component.to_html().into())
        .unwrap()
}
