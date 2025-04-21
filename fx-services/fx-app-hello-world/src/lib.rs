use {
    fx::{FxCtx, HttpRequest, HttpResponse, FetchRequest, rpc},
    tracing::info,
    serde::{Serialize, Deserialize},
};

#[rpc]
pub fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    info!("hello from wasm service!");

    let kv = ctx.kv("demo");
    let counter: i64 = ctx.rpc("counter", "incr", ());

    let instance = kv.get("instance").map(|v| String::from_utf8(v).unwrap());

    if req.url == "/test-rpc" {
        let response: RpcResponse = ctx.rpc("rpc-test-service", "hello", RpcRequest { number: 42 });
        return HttpResponse {
            body: format!("rpc demo returned a response: {response:?}\n"),
        };
    } else if req.url == "/test-fetch" {
        let res = ctx.fetch(FetchRequest::get("http://httpbin.org/get".to_owned()));
        return HttpResponse { body: String::from_utf8(res.body).unwrap() };
    }

    HttpResponse::new().body(format!("Hello from {:?} rpc style, counter value using global: {counter:?}, instance: {instance:?}", req.url))
}

#[derive(Serialize)]
struct RpcRequest {
    number: i64,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct RpcResponse {
    number: i64,
}
