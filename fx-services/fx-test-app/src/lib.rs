use {
    std::{time::Duration, collections::HashMap, sync::Mutex},
    tracing::info,
    fx::{rpc, FxCtx, SqlQuery, sleep, HttpRequest, FxStream, FxStreamExport, KvError, fetch, metrics::Counter},
    fx_runtime_common::FunctionInvokeEvent,
    lazy_static::lazy_static,
};

lazy_static! {
    static ref COUNTER: Mutex<u64> = Mutex::new(0);
    static ref INVOCATION_COUNT: Mutex<HashMap<String, u64>> = Mutex::new(HashMap::new());
}

#[rpc]
pub async fn simple(_ctx: &FxCtx, arg: u32) -> u32 {
    arg + 42
}

#[rpc]
pub async fn sql_simple(ctx: &FxCtx, _arg: ()) -> u64 {
    ctx.init_logger();
    let database = ctx.sql("app");
    database.exec(SqlQuery::new("create table test_sql_simple (v integer not null)")).unwrap();
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (42)")).unwrap();
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (10)")).unwrap();
    let res = database.exec(SqlQuery::new("select sum(v) from test_sql_simple")).unwrap().into_rows().first().unwrap().columns.first().unwrap().try_into().unwrap();
    database.exec(SqlQuery::new("drop table test_sql_simple")).unwrap();
    res
}

#[rpc]
pub async fn async_simple(ctx: &FxCtx, arg: u64) -> u64 {
    ctx.init_logger();
    sleep(Duration::from_secs(3)).await;
    arg
}

#[rpc]
pub async fn rpc_responder(ctx: &FxCtx, arg: u64) -> u64 {
    ctx.init_logger();
    sleep(Duration::from_secs(1)).await;
    arg * 2
}

#[rpc]
pub async fn call_rpc(ctx: &FxCtx, arg: u64) -> u64 {
    ctx.init_logger();
    ctx.rpc("other-app", "rpc_responder", arg).await.unwrap()
}

#[rpc]
pub async fn rpc_responder_panic(ctx: &FxCtx, _arg: ()) -> u64 {
    ctx.init_logger();
    panic!("test panic");
}

#[rpc]
pub async fn rpc_responder_panic_async(ctx: &FxCtx, _arg: ()) -> u64 {
    ctx.init_logger();
    sleep(Duration::from_secs(1)).await;
    panic!("test panic");
}

#[rpc]
pub async fn call_rpc_panic(ctx: &FxCtx, _arg: ()) -> i64 {
    ctx.init_logger();
    let res0 = match ctx.rpc::<(), u64>("other-app", "rpc_responder_panic", ()).await {
        Ok(_) => panic!("didn't expect test rpc call not to fail"),
        Err(_) => 0,
    };
    let res1 = match ctx.rpc::<(), u64>("other-app", "rpc_responder_panic_async", ()).await {
        Ok(_) => panic!("didn't expect test rpc call not to fail"),
        Err(_) => 0,
    };

    42 + res0 as i64 + res1 as i64
}

#[rpc]
pub async fn test_fetch(ctx: &FxCtx, _arg: ()) -> Result<String, String> {
    ctx.init_logger();
    let response = fetch(
        HttpRequest::get("https://fx.nikitavbv.com/api/mock/get").unwrap()
    ).await.unwrap();

    if !response.status.is_success() {
        return Err(format!("mock endpoint returned unexpected status code: {:?}, request id: {:?}", response.status, response.headers().get("x-request-id")));
    }

    Ok(String::from_utf8(response.body).unwrap())
}

#[rpc]
pub async fn global_counter_inc(_ctx: &FxCtx, _arg: ()) -> u64 {
    let mut counter = COUNTER.lock().unwrap();
    *counter += 1;
    *counter
}

#[rpc]
pub async fn on_invoke(_ctx: &FxCtx, event: FunctionInvokeEvent) {
    /*let mut invocation_count = INVOCATION_COUNT.lock().unwrap();
    let count = invocation_count.get(&event.function_id).unwrap_or(&0) + 1;
    invocation_count.insert(event.function_id, count);*/
}

#[rpc]
pub async fn get_invoke_count(_ctx: &FxCtx, function_id: String) -> u64 {
    *INVOCATION_COUNT.lock().unwrap().get(&function_id).unwrap_or(&0)
}

#[rpc]
pub async fn test_panic(ctx: &FxCtx, _arg: ()) {
    ctx.init_logger();
    panic!("test panic");
}

#[rpc]
pub async fn test_stream_simple(ctx: &FxCtx, _arg: ()) -> FxStream {
    ctx.init_logger();

    let stream = async_stream::stream! {
        for i in 0..5 {
            yield vec![i];
            sleep(Duration::from_secs(1)).await;
        }
    };
    FxStream::wrap(stream).unwrap()
}

#[rpc]
pub async fn test_random(ctx: &FxCtx, len: u64) -> Vec<u8> {
    ctx.init_logger();
    ctx.random(len)
}

#[rpc]
pub async fn test_time(ctx: &FxCtx, _arg: ()) -> u64 {
    ctx.init_logger();
    let started_at = ctx.now();
    sleep(Duration::from_secs(1)).await;
    (ctx.now() - started_at).as_millis() as u64
}

#[rpc]
pub async fn test_kv_set(ctx: &FxCtx, value: String) {
    ctx.init_logger();
    let kv = ctx.kv("test-kv");
    kv.set("test-key", value.as_bytes()).unwrap();
}

#[rpc]
pub async fn test_kv_get(ctx: &FxCtx, _arg: ()) -> Option<String> {
    ctx.init_logger();
    let kv = ctx.kv("test-kv");
    kv.get("test-key").unwrap().map(|v| String::from_utf8(v).unwrap())
}

#[rpc]
pub async fn test_kv_wrong_binding_name(ctx: &FxCtx, _arg: ()) {
    ctx.init_logger();
    let kv = ctx.kv("test-kv-wrong");
    let err = kv.set("test-key", "hello world!".as_bytes()).err().unwrap();
    assert_eq!(KvError::BindingDoesNotExist, err);
}

#[rpc]
pub async fn test_log(ctx: &FxCtx, _arg: ()) {
    ctx.init_logger();
    info!("this is a test log");
}

#[rpc]
pub async fn test_log_span(ctx: &FxCtx, _arg: ()) {
    ctx.init_logger();
    let span = tracing::info_span!("test_log_span", request_id="some-request-id");
    let _guard = span.enter();

    info!("first message");
    info!("second message");
}

#[rpc]
pub async fn test_counter_increment(ctx: &FxCtx, _arg: ()) {
    ctx.init_logger();
    Counter::new("test_counter").increment(1);
}

#[rpc]
pub async fn test_counter_increment_twice_with_tags(ctx: &FxCtx, _arg: ()) {
    ctx.init_logger();
    Counter::new_with_tags("test_counter_with_label", vec!["label_name".to_owned()]).increment_with_tag_values(vec!["value1".to_owned()], 1);
    Counter::new_with_tags("test_counter_with_label", vec!["label_name".to_owned()]).increment_with_tag_values(vec!["value2".to_owned()], 1);
}
