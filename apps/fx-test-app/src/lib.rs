use {
    std::{time::Duration, collections::HashMap, sync::Mutex},
    tracing::info,
    fx_sdk::{self as fx, handler, SqlQuery, sleep, HttpRequest, HttpRequestV2, HttpResponse, FxStream, FxStreamExport, KvError, fetch, metrics::Counter},
    lazy_static::lazy_static,
};

mod unknown_import;

lazy_static! {
    static ref COUNTER: Mutex<u64> = Mutex::new(0);
    static ref INVOCATION_COUNT: Mutex<HashMap<String, u64>> = Mutex::new(HashMap::new());
}

#[handler::fetch]
pub async fn http(_req: HttpRequestV2) -> HttpResponse {
    HttpResponse::new().with_body("hello fx!")
}

#[handler]
pub async fn simple(arg: u32) -> fx::Result<u32> {
    Ok(arg + 42)
}

#[handler]
pub async fn sql_simple() -> fx::Result<u64> {
    let database = fx::sql("app");
    database.exec(SqlQuery::new("create table test_sql_simple (v integer not null)")).unwrap();
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (42)")).unwrap();
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (10)")).unwrap();
    let res = database.exec(SqlQuery::new("select sum(v) from test_sql_simple")).unwrap().into_rows().first().unwrap().columns.first().unwrap().try_into().unwrap();
    database.exec(SqlQuery::new("drop table test_sql_simple")).unwrap();
    Ok(res)
}

#[handler]
pub async fn async_simple(arg: u64) -> fx::Result<u64> {
    sleep(Duration::from_secs(3)).await;
    Ok(arg)
}

#[handler]
pub async fn rpc_responder(arg: u64) -> fx::Result<u64> {
    sleep(Duration::from_secs(1)).await;
    Ok(arg * 2)
}

#[handler]
pub async fn rpc_responder_panic() -> fx::Result<u64> {
    panic!("test panic");
}

#[handler]
pub async fn rpc_responder_panic_async() -> fx::Result<u64> {
    sleep(Duration::from_secs(1)).await;
    panic!("test panic");
}

#[handler]
pub async fn test_fetch() -> fx::Result<Result<String, String>> {
    let response = fetch(
        HttpRequest::get("https://fx.nikitavbv.com/api/mock/get").unwrap()
    ).await.unwrap();

    if !response.status.is_success() {
        return Ok(Err(format!("mock endpoint returned unexpected status code: {:?}, request id: {:?}", response.status, response.headers().get("x-request-id"))));
    }

    Ok(Ok(String::from_utf8(response.body).unwrap()))
}

#[handler]
pub async fn global_counter_inc() -> fx::Result<u64> {
    let mut counter = COUNTER.lock().unwrap();
    *counter += 1;
    Ok(*counter)
}

#[handler]
pub async fn get_invoke_count(function_id: String) -> fx::Result<u64> {
    Ok(*INVOCATION_COUNT.lock().unwrap().get(&function_id).unwrap_or(&0))
}

#[handler]
pub async fn test_panic() -> fx::Result<()> {
    panic!("test panic");
}

#[handler]
pub async fn test_stream_simple() -> fx::Result<FxStream> {
    let stream = async_stream::stream! {
        for i in 0..5 {
            yield vec![i];
            sleep(Duration::from_secs(1)).await;
        }
    };
    Ok(FxStream::wrap(stream).unwrap())
}

#[handler]
pub async fn test_random(len: u64) -> fx::Result<Vec<u8>> {
    Ok(fx::random(len))
}

#[handler]
pub async fn test_time() -> fx::Result<u64> {
    let started_at = fx::now();
    sleep(Duration::from_secs(1)).await;
    Ok((fx::now() - started_at).as_millis() as u64)
}

#[handler]
pub async fn test_kv_set(value: String) -> fx::Result<()> {
    let kv = fx::kv("test-kv");
    kv.set("test-key", value.as_bytes()).unwrap();
    Ok(())
}

#[handler]
pub async fn test_kv_get() -> fx::Result<Option<String>> {
    let kv = fx::kv("test-kv");
    Ok(kv.get("test-key").unwrap().map(|v| String::from_utf8(v).unwrap()))
}

#[handler]
pub async fn test_kv_wrong_binding_name() -> fx::Result<()> {
    let kv = fx::kv("test-kv-wrong");
    let err = kv.set("test-key", "hello world!".as_bytes()).err().unwrap();
    assert_eq!(KvError::BindingDoesNotExist, err);
    Ok(())
}

#[handler]
pub async fn test_log() -> fx::Result<()> {
    info!("this is a test log");
    Ok(())
}

#[handler]
pub async fn test_log_span() -> fx::Result<()> {
    let span = tracing::info_span!("test_log_span", request_id="some-request-id");
    let _guard = span.enter();

    info!("first message");
    info!("second message");

    Ok(())
}

#[handler]
pub async fn test_counter_increment() -> fx::Result<()> {
    Counter::new("test_counter").increment(1);
    Ok(())
}

#[handler]
pub async fn test_counter_increment_twice_with_tags() -> fx::Result<()> {
    Counter::new_with_tags("test_counter_with_label", vec!["label_name".to_owned()]).increment_with_tag_values(vec!["value1".to_owned()], 1);
    Counter::new_with_tags("test_counter_with_label", vec!["label_name".to_owned()]).increment_with_tag_values(vec!["value2".to_owned()], 1);
    Ok(())
}

#[handler]
pub async fn test_unknown_import() -> fx::Result<()> {
    unsafe { crate::unknown_import::_test_unknown_import(-1) };
    Ok(())
}
