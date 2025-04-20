use {
    fx::{FxCtx, HttpRequest, HttpResponse, handler},
    tracing::info,
};

#[handler]
pub fn handle(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    info!("hello from wasm service!");

    let kv = ctx.kv("demo");
    let counter = kv.get("counter").map(|v| String::from_utf8(v).unwrap().parse().unwrap()).unwrap_or(0);
    let counter = counter + 1;
    kv.set("counter", counter.to_string().as_bytes());

    HttpResponse {
        body: format!("Hello from {:?}, counter value: {counter:?}", req.url),
    }
}
