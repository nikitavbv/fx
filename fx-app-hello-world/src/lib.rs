use {
    fx::{FxCtx, HttpRequest, HttpResponse, handler},
    tracing::info,
};

#[handler]
pub fn handle(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    info!("hello from wasm service!");

    let kv = ctx.kv("demo");
    let v = String::from_utf8(kv.get("counter")).unwrap();

    HttpResponse {
        body: format!("Hello from {:?}, counter value: {v:?}", req.url),
    }
}
