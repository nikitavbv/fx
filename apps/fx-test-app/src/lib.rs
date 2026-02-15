use {
    std::{time::Duration, collections::HashMap, sync::Mutex},
    tracing::info,
    axum::{Router, routing::get, Extension},
    lazy_static::lazy_static,
    fx_sdk::{
        self as fx,
        handler,
        SqlQuery,
        sleep,
        HttpRequest,
        HttpResponse,
        io::http::fetch,
        StatusCode,
        utils::{axum::handle_request, migrations::{Migrations, Migration}},
        random,
        io::blob::BlobGetError,
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
pub async fn http(req: HttpRequest) -> HttpResponse {
    if req.uri().path().starts_with("/test/http/header-get-simple") {
        return HttpResponse::new().with_body(format!("ok: {}\n", req.headers().get("x-test-header").unwrap().to_str().unwrap()))
    }

    handle_request(
        Router::new()
            .route("/test/status-code", get(test_status_code))
            .route("/test/sql/simple", get(test_sql_simple))
            .route("/test/sql/migrate", get(test_sql_migrate))
            .route("/test/panic", get(test_panic_page))
            .route("/test/sleep", get(test_sleep))
            .route("/test/random", get(test_random))
            .route("/test/time", get(test_time))
            .route("/test/blob", get(test_blob_get).post(test_blob_put).delete(test_blob_delete))
            .route("/test/blob/wrong-binding-name", get(test_blob_wrong_binding_name))
            .route("/test/fetch", get(test_fetch))
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
        .await;

    let res: u64 = database.exec(SqlQuery::new("select v from test_sql_table_migrations")).await.unwrap().into_rows().first().unwrap().columns.first().unwrap().try_into().unwrap();
    res.to_string()
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

    String::from_utf8(response.body).unwrap()
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
