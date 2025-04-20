use {
    fx::{FxCtx, HttpRequest, HttpResponse, handler},
    tracing::info,
    bincode::{Encode, Decode},
};

#[handler]
pub fn handle(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    info!("hello from wasm service!");

    let kv = ctx.kv("demo");
    let counter = kv.get("counter").map(|v| String::from_utf8(v).unwrap().parse().unwrap()).unwrap_or(0);
    let counter = counter + 1;
    kv.set("counter", counter.to_string().as_bytes());

    let instance = kv.get("instance").map(|v| String::from_utf8(v).unwrap());

    if req.url == "/test-rpc" {
        let response: RpcResponse = ctx.rpc("rpc-test-service", "hello", RpcRequest { number: 42 });
        return HttpResponse {
            body: format!("rpc demo returned a response: {response:?}\n"),
        };
    }

    HttpResponse {
        body: format!("Hello from {:?}, counter value: {counter:?}, instance: {instance:?}", req.url),
    }
}

#[derive(Encode)]
struct RpcRequest {
    number: i64,
}

#[derive(Decode, Debug)]
struct RpcResponse {
    number: i64,
}
