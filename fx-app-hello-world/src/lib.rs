use {
    fx::{FxCtx, HttpRequest, HttpResponse, handler},
    tracing::info,
};

#[handler]
pub fn handle(ctx: FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    info!("hello from wasm service!");

    HttpResponse {
        body: format!("Hello from {:?}", req.url),
    }
}
