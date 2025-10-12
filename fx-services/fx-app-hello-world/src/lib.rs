use {
    fx::{FxCtx, HttpRequest, HttpResponse, rpc, SqlQuery, fetch},
    tracing::info,
    serde::{Serialize, Deserialize},
};

#[rpc]
pub async fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    info!("hello from wasm service!");

    // let kv = ctx.kv("demo");
    //let counter: i64 = ctx.rpc("counter", "incr", ()).await.unwrap();
    let counter = 42;

    // let instance = kv.get("instance").unwrap().map(|v| String::from_utf8(v).unwrap());
    let instance = "demo";

    if req.url == "/test-rpc" {
        let response: RpcResponse = ctx.rpc("rpc-test-service", "hello", RpcRequest { number: 42 }).await.unwrap();
        return HttpResponse::new().with_body(format!("rpc demo returned a response: {response:?}\n"));
    } else if req.url == "/test-fetch" {
        let res = fetch(HttpRequest::get("http://httpbin.org/get".to_owned()).unwrap()).await.unwrap();
        return HttpResponse::new().with_body(String::from_utf8(res.body).unwrap());
    } else if req.url == "/test-sql" {
        let database = ctx.sql("test-db");
        database.exec(SqlQuery::new("create table if not exists hello_table (v integer not null)")).unwrap();
        database.exec(SqlQuery::new("insert into hello_table (v) values (?)").bind(42)).unwrap();
        let result = database.exec(SqlQuery::new("select sum(v) as total from hello_table")).unwrap();
        return HttpResponse::new().with_body(format!("hello sql! x={:?}", result.into_rows()[0].columns[0]));
    }

    HttpResponse::new().with_body(format!("Hello from {:?} rpc style, counter value using global: {counter:?}, instance: {instance:?}", req.url))
}

#[rpc]
pub fn hello_cron(ctx: &FxCtx, _req: ()) {
    ctx.init_logger();
    info!("hello from cron!");
}

#[rpc]
pub fn example(ctx: &FxCtx, _req: ()) {
    ctx.init_logger();
    info!("hello from fx!");
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
