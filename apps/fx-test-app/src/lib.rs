use {
    std::{time::Duration, collections::HashMap, sync::Mutex},
    tracing::info,
    axum::{Router, routing::{get, post}, Extension},
    lazy_static::lazy_static,
    fx_sdk::{
        self as fx,
        handler,
        SqlQuery,
        sleep,
        HttpRequest,
        HttpResponse,
        io::{http::fetch, blob::BlobGetError},
        StatusCode,
        utils::{axum::handle_request, migrations::{Migrations, Migration}},
        random,
        blob,
        metrics::Counter,
    },
};

mod unknown_import;

lazy_static! {
    static ref COUNTER: Mutex<u64> = Mutex::new(0);
    static ref INVOCATION_COUNT: Mutex<HashMap<String, u64>> = Mutex::new(HashMap::new());
}

#[handler::fetch]
pub async fn http(mut req: HttpRequest) -> HttpResponse {
    let req = if req.uri().path().starts_with("/test/http/header-get-simple") {
        return HttpResponse::new().with_body(format!("ok: {}\n", req.headers().get("x-test-header").unwrap().to_str().unwrap()))
    } else if req.uri().path().starts_with("/test/http/uri-overwrite") {
        req.with_uri("http://localhost:8080/test/http/uri-overwritten".parse().unwrap())
    } else if req.uri().path().starts_with("/test/fetch/body-passthrough") {
        let body = req.body().unwrap();
        return fetch(req.with_uri("https://httpbin.org/post".parse().unwrap()).with_body(body)).await.unwrap();
    } else {
        req
    };

    handle_request(
        Router::new()
            .route("/test/status-code", get(test_status_code))
            .route("/test/http/uri-overwritten", get(test_uri_overwritten))
            .route("/test/http/body", post(test_http_body))
            .route("/test/sql/simple", get(test_sql_simple))
            .route("/test/sql/migrate", get(test_sql_migrate))
            .route("/test/sql/contention/setup", get(test_sql_contention_setup))
            .route("/test/sql/contention/expensive-write", get(test_sql_contention_expensive_write))
            .route("/test/sql/contention/read", get(test_sql_contention_read))
            .route("/test/panic", get(test_panic_page))
            .route("/test/sleep", get(test_sleep))
            .route("/test/random", get(test_random))
            .route("/test/time", get(test_time))
            .route("/test/blob", get(test_blob_get).post(test_blob_put).delete(test_blob_delete))
            .route("/test/blob/wrong-binding-name", get(test_blob_wrong_binding_name))
            .route("/test/fetch", get(test_fetch))
            .route("/test/fetch/post", get(test_fetch_post))
            .route("/test/fetch/query", get(test_fetch_query))
            .route("/test/log", get(test_log))
            .route("/test/log/span", get(test_log_span))
            .route("/test/metrics/counter-increment", get(test_metrics_counter_increment))
            .route("/test/metrics/counter-with-labels-increment", get(test_metrics_counter_with_labels_increment))
            .route("/test/unknown-import", get(test_unknown_import))
            .route("/", get(home))
            .layer(Extension(Metrics::new())),
        req
    ).await
}

async fn home() -> &'static str {
    "hello fx!"
}

async fn test_status_code() -> (StatusCode, &'static str) {
    (StatusCode::IM_A_TEAPOT, "this returns custom status code.\n")
}

async fn test_uri_overwritten() -> &'static str {
    "hello from overwritten uri"
}

async fn test_http_body(body_text: String) -> String {
    body_text.chars().rev().collect()
}

async fn test_sql_simple() -> String {
    let database = fx::sql("app");
    database.exec(SqlQuery::new("create table test_sql_simple (v integer not null)")).await.unwrap();
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (42)")).await.unwrap();
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (10)")).await.unwrap();
    let res: u64 = database.exec(SqlQuery::new("select sum(v) from test_sql_simple")).await.unwrap().into_rows().first().unwrap().columns.first().unwrap().try_into().unwrap();
    database.exec(SqlQuery::new("drop table test_sql_simple")).await.unwrap();
    res.to_string()
}

async fn test_sql_migrate() -> String {
    let database = fx::sql("app");

    Migrations::new()
        .with_migration(Migration::new("create table test_sql_table_migrations (v integer not null)"))
        .with_migration(Migration::new("insert into test_sql_table_migrations (v) values (67)"))
        .run(&database)
        .await
        .unwrap();

    let res: u64 = database.exec(SqlQuery::new("select v from test_sql_table_migrations")).await.unwrap().into_rows().first().unwrap().columns.first().unwrap().try_into().unwrap();
    res.to_string()
}

async fn test_sql_contention_setup() -> &'static str {
    let database = fx::sql("contention-test");

    Migrations::new()
        .with_migration(Migration::new("create table simple_contention (v blob not null)"))
        .run(&database)
        .await
        .unwrap();

    "ok.\n"
}

async fn test_sql_contention_expensive_write() -> &'static str {
    let database = fx::sql("contention-test");

    database.exec(SqlQuery::new("with recursive cnt(x) AS (select 1 union all select x + 1 from cnt where x < 1000) insert into simple_contention (v) select randomblob(10000) from cnt")).await.unwrap();
    database.exec(SqlQuery::new("delete from simple_contention where 1")).await.unwrap();

    "ok.\n"
}

async fn test_sql_contention_read() -> String {
    let database = fx::sql("contention-test");

    let rows = database.exec(SqlQuery::new("select v from simple_contention limit 1")).await.unwrap().into_rows();
    if rows.is_empty() {
        return "ok.\nempty".to_owned();
    }

    let v = match &rows[0].columns[0] {
        fx_sdk::SqlValue::Blob(v) => v.len(),
        _other => panic!("unexpected sql value"),
    };

    format!("ok.\n{v}")
}

async fn test_panic_page() -> String {
    panic!("test function panic")
}

async fn test_sleep() -> &'static str {
    sleep(Duration::from_secs(3)).await;
    "slept for a few seconds"
}

async fn test_random() -> String {
    use base64::prelude::*;
    BASE64_STANDARD.encode(random(32))
}

async fn test_time() -> String {
    let started_at = fx::now();
    sleep(Duration::from_secs(1)).await;
    (fx::now() - started_at).as_millis().to_string()
}

async fn test_blob_get() -> (StatusCode, String) {
    match blob("test-blob").get("test-key".to_owned()).await.unwrap() {
        None => (StatusCode::NOT_FOUND, "key not set".to_owned()),
        Some(v) => (StatusCode::OK, String::from_utf8(v).unwrap())
    }
}

async fn test_blob_put() {
    blob("test-blob").put("test-key".to_owned(), "test-value".as_bytes().to_vec()).await;
}

async fn test_blob_delete() {
    blob("test-blob").delete("test-key".to_owned()).await;
}

async fn test_blob_wrong_binding_name() -> (StatusCode, String) {
    match blob("test-blob-wrong-binding-name").get("test-key".to_owned()).await {
        Err(BlobGetError::BindingNotExists) => (StatusCode::OK, "got BindingNotExists, as expected".to_owned()),
        other => (StatusCode::INTERNAL_SERVER_ERROR, format!("unexpected result when getting key from binding that does not exist: {other:?}"))
    }
}

async fn test_fetch() -> String {
    let response = fetch(
        HttpRequest::get("https://httpbin.org/get").unwrap()
    ).await.unwrap();

    String::from_utf8(response.into_body()).unwrap()
}

async fn test_fetch_post() -> String {
    let response = fetch(
        HttpRequest::post("https://httpbin.org/post").unwrap().with_body("test fx request body")
    ).await.unwrap();

    String::from_utf8(response.into_body()).unwrap()
}

async fn test_fetch_query() -> String {
    let response = fetch(
        HttpRequest::get("https://httpbin.org/get").unwrap()
            .with_query(&[("param1", "value1"), ("param2", "value2")])
    ).await.unwrap();

    String::from_utf8(response.into_body()).unwrap()
}

async fn test_log() -> &'static str {
    info!("this is a test log");
    "ok.\n"
}

async fn test_log_span() -> &'static str {
    let span = tracing::info_span!("test_log_span", request_id="some-request-id");
    let _guard = span.enter();

    info!("first message");
    info!("second message");

    "ok.\n"
}

async fn test_metrics_counter_increment(Extension(metrics): Extension<Metrics>) -> &'static str {
    metrics.test_counter.increment();
    "ok.\n"
}

async fn test_metrics_counter_with_labels_increment(Extension(metrics): Extension<Metrics>) -> &'static str {
    metrics.test_counter_with_label1.increment();
    metrics.test_counter_with_label2.increment_by(2);
    "ok.\n"
}

async fn test_unknown_import() -> &'static str {
    unsafe { crate::unknown_import::_test_unknown_import(-1) };
    "ok.\n"
}

#[derive(Clone)]
struct Metrics {
    test_counter: Counter,
    test_counter_with_label1: Counter,
    test_counter_with_label2: Counter,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            test_counter: Counter::new("test_counter"),
            test_counter_with_label1: Counter::new_with_labels(
                "test_counter_with_label",
                vec!["label_name".to_owned()]
            ).with_label_values(vec!["value1".to_owned()]),
            test_counter_with_label2: Counter::new_with_labels(
                "test_counter_with_label",
                vec!["label_name".to_owned()]
            ).with_label_values(vec!["value2".to_owned()]),
        }
    }
}
