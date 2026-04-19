use {
    std::{time::Duration, collections::HashMap, sync::Mutex, convert::Infallible},
    tracing::info,
    axum::{Router, routing::{get, post}, Extension, response::sse::{Sse, Event}, extract::Json},
    lazy_static::lazy_static,
    futures::stream::{self, Stream, StreamExt},
    serde::Deserialize,
    stream_reduce::Reduce,
    sha2::{Sha256, Digest},
    base64::prelude::*,
    fx_sdk::{
        self as fx,
        handler,
        SqlQuery,
        sleep,
        HttpRequest,
        HttpResponse,
        io::{http::{fetch, FetchError, HttpBody}, blob::BlobGetError, kv::{self, KvSetNxPxError}, env, tasks},
        StatusCode,
        utils::{axum::handle_request, migrations::{Migrations, Migration, SqlMigrationError}},
        random,
        blob,
        metrics::Counter,
        sql::{SqlError, SqlValue},
        SqlDatabase,
    },
};

mod unknown_import;

lazy_static! {
    static ref COUNTER: Mutex<u64> = Mutex::new(0);
    static ref INVOCATION_COUNT: Mutex<HashMap<String, u64>> = Mutex::new(HashMap::new());
}

static mut PANIC_RESTORE_COUNTER: u32 = 0;

#[handler]
pub async fn http(mut req: HttpRequest) -> HttpResponse {
    let req = if req.uri().path().starts_with("/test/http/header-get-simple") {
        return HttpResponse::new().with_body(format!("ok: {}\n", req.headers().get("x-test-header").unwrap().to_str().unwrap()))
    } else if req.uri().path().starts_with("/test/http/uri-overwrite") {
        req.with_uri("http://localhost:8080/test/http/uri-overwritten".parse().unwrap())
    } else if req.uri().path().starts_with("/test/fetch/body-passthrough") {
        let body = req.body().unwrap();
        return fetch(req.with_uri("https://httpbin.org/post".parse().unwrap()).with_body(body).without_header(&axum::http::header::HOST)).await.unwrap();
    } else {
        req
    };

    handle_request(
        Router::new()
            .route("/test/status-code", get(test_status_code))
            .route("/test/http/uri-overwritten", get(test_uri_overwritten))
            .route("/test/http/response-header", get(test_response_header))
            .route("/test/http/body", post(test_http_body))
            .route("/test/sql/simple", get(test_sql_simple))
            .route("/test/sql/migrate", get(test_sql_migrate))
            .route("/test/sql/contention/setup", get(test_sql_contention_setup))
            .route("/test/sql/contention/expensive-write", get(test_sql_contention_expensive_write))
            .route("/test/sql/contention/read", get(test_sql_contention_read))
            .route("/test/sql/contention-busy", get(test_sql_contention_busy))
            .route("/test/sql/wrong-binding-name", get(test_sql_wrong_binding_name))
            .route("/test/sql/wrong-binding-name/migrations", get(test_sql_wrong_binding_name_migrations))
            .route("/test/sql/migration-sql-error", get(test_sql_migration_sql_error))
            .route("/test/sql/nonexistent-db", get(test_sql_nonexistent_db))
            .route("/test/sql/batch", get(test_sql_batch))
            .route("/test/sql/batch-rollback", get(test_sql_batch_rollback))
            .route("/test/panic", get(test_panic_page))
            .route("/test/panic-restore", get(test_panic_restore))
            .route("/test/panic-restore/test", get(test_panic_restore_test))
            .route("/test/sleep", get(test_sleep))
            .route("/test/random", get(test_random))
            .route("/test/time", get(test_time))
            .route("/test/blob", get(test_blob_get).post(test_blob_put).delete(test_blob_delete))
            .route("/test/blob/wrong-binding-name", get(test_blob_wrong_binding_name))
            .route("/test/fetch", get(test_fetch))
            .route("/test/fetch/post", get(test_fetch_post))
            .route("/test/fetch/query", get(test_fetch_query))
            .route("/test/fetch/with-header", get(test_fetch_with_header))
            .route("/test/fetch/body-read-all", get(test_fetch_body_read_all))
            .route("/test/fetch/timeout", get(test_fetch_timeout))
            .route("/test/log", get(test_log))
            .route("/test/log/span", get(test_log_span))
            .route("/test/metrics/counter-increment", get(test_metrics_counter_increment))
            .route("/test/metrics/counter-with-labels-increment", get(test_metrics_counter_with_labels_increment))
            .route("/test/unknown-import", get(test_unknown_import))
            .route("/test/cron", get(read_cron_status))
            .route("/test/cron/custom-endpoint", get(read_cron_custom_endpoint_status))
            .route("/test/rpc/simple", get(rpc_simple))
            .route("/test/stream/sse", get(test_stream_sse))
            .route("/test/env/simple", get(env_simple))
            .route("/test/env/missing-file", get(env_missing_file))
            .route("/test/kv/simple", get(kv_simple))
            .route("/test/kv/distributed-lock", get(kv_distributed_lock))
            .route("/test/kv/pubsub/subscribe", get(kv_pubsub_subscribe))
            .route("/test/kv/pubsub/publish", post(kv_pubsub_publish))
            .route("/test/tasks/background/start", post(kv_tasks_background_start))
            .route("/test/tasks/background/status", get(kv_tasks_background_status))
            .route("/test/limits/memory", get(test_limits_memory))
            .route("/test/limits/cpu-preemption", get(test_cpu_preemption))
            .route("/_fx/cron", get(handle_cron))
            .route("/_fx/cron/custom-endpoint-for-task", get(handle_cron_custom_endpoint_for_task))
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

async fn test_response_header() -> (axum::http::StatusCode, axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::HeaderName::from_static("x-custom-header"),
        axum::http::HeaderValue::from_static("test-value"),
    );
    (axum::http::StatusCode::OK, headers, "response with custom header")
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

async fn test_sql_contention_busy() -> &'static str {
    let database = fx::sql("contention-busy");

    let result = Migrations::new()
        .with_migration(Migration::new("create table contention_busy (v blob not null)"))
        .run(&database)
        .await;
    if let Err(err) = result {
        match err {
            SqlMigrationError::DatabaseBusy => return "busy.\n",
            SqlMigrationError::MigrationExecutionError { message: _ } => return "migration execution error.\n",
            other => panic!("unexpected error: {other:?}"),
        }
    }

    let result = database.exec(SqlQuery::new("with recursive cnt(x) AS (select 1 union all select x + 1 from cnt where x < 1000) insert into contention_busy (v) select randomblob(10000) from cnt")).await;
    if let Err(err) = result {
        match err {
            SqlError::DatabaseBusy => return "busy.\n",
            other => panic!("unexpected error: {other:?}"),
        }
    }
    let result = database.exec(SqlQuery::new("delete from contention_busy where 1")).await;
    if let Err(err) = result {
        match err {
            SqlError::DatabaseBusy => return "busy.\n",
            other => panic!("unexpected error: {other:?}"),
        }
    }

    "ok.\n"
}

async fn test_sql_wrong_binding_name() -> (StatusCode, &'static str) {
    let database = fx::sql("wrong-binding-name");

    match database.exec(SqlQuery::new("select 1")).await {
        Ok(_) => (StatusCode::INTERNAL_SERVER_ERROR, "didn't expect sql query not to fail."),
        Err(err) => match err {
            SqlError::BindingNotFound => (StatusCode::OK, "ok: binding not found.\n"),
            other => panic!("unexpected error type: {other:?}"),
        }
    }
}

async fn test_sql_migration_sql_error() -> (StatusCode, String) {
    let database = fx::sql("migration-sql-error");

    // to trigger "table already exists" error below
    database.exec(SqlQuery::new("create table test_migration_sql_error (v integer not null)")).await.unwrap();

    let result = Migrations::new()
        .with_migration(Migration::new("create table test_migration_sql_error (v integer not null)"))
        .run(&database)
        .await;

    match result {
        Ok(_) => (StatusCode::INTERNAL_SERVER_ERROR, "didn't expect migration to succeed.".to_owned()),
        Err(err) => match err {
            SqlMigrationError::SqlError { message } => (StatusCode::OK, format!("ok: migration sql error: {message}\n")),
            other => panic!("unexpected error type: {other:?}"),
        }
    }
}

async fn test_sql_wrong_binding_name_migrations() -> (StatusCode, &'static str) {
    let database = fx::sql("wrong-binding-name");


    let result = Migrations::new()
        .with_migration(Migration::new("create table test_table (v integer not null)"))
        .run(&database)
        .await;

    match result {
        Ok(_) => (StatusCode::INTERNAL_SERVER_ERROR, "didn't expect sql query not to fail."),
        Err(err) => match err {
            SqlMigrationError::BindingNotFound => (StatusCode::OK, "ok: binding not found.\n"),
            other => panic!("unexpected error type: {other:?}"),
        }
    }
}

async fn test_sql_nonexistent_db() -> (StatusCode, String) {
    let database = fx::sql("nonexistent-db");
    let result = database.exec(SqlQuery::new("SELECT 1")).await;
    match result {
        Ok(rows) => {
            match rows.into_rows().first().unwrap().columns.first().unwrap() {
                SqlValue::Integer(v) => (StatusCode::OK, format!("ok: {}", v)),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "error: unexpected type".to_owned()),
            }
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {:?}", err)),
    }
}

async fn test_sql_batch() -> (StatusCode, String) {
    let database = fx::sql("app");

    database.exec(SqlQuery::new("drop table if exists test_batch_table")).await.unwrap();

    let queries = vec![
        SqlQuery::new("create table test_batch_table (id integer primary key, value integer not null)"),
        SqlQuery::new("insert into test_batch_table (value) values (100)"),
        SqlQuery::new("insert into test_batch_table (value) values (200)"),
        SqlQuery::new("insert into test_batch_table (value) values (300)"),
    ];

    match database.batch(queries).await {
        Ok(()) => {
            match database.exec(SqlQuery::new("select sum(value) from test_batch_table")).await {
                Ok(result) => {
                    let sum: i64 = result.into_rows().first().unwrap().columns.first().unwrap().try_into().unwrap();
                    let _ = database.exec(SqlQuery::new("drop table test_batch_table")).await;
                    if sum == 600 {
                        (StatusCode::OK, format!("sum={}", sum))
                    } else {
                        (StatusCode::INTERNAL_SERVER_ERROR, format!("expected sum=600 but got {}", sum))
                    }
                }
                Err(err) => {
                    let _ = database.exec(SqlQuery::new("drop table test_batch_table")).await;
                    (StatusCode::INTERNAL_SERVER_ERROR, format!("verify error: {:?}", err))
                }
            }
        }
        Err(err) => {
            let _ = database.exec(SqlQuery::new("drop table test_batch_table")).await;
            (StatusCode::INTERNAL_SERVER_ERROR, format!("batch error: {:?}", err))
        }
    }
}

async fn test_sql_batch_rollback() -> (StatusCode, String) {
    let database = fx::sql("app");

    database.exec(SqlQuery::new("drop table if exists test_rollback_table")).await.unwrap();
    database.exec(SqlQuery::new("create table test_rollback_table (id integer primary key, value integer not null)")).await.unwrap();
    database.exec(SqlQuery::new("insert into test_rollback_table (value) values (42)")).await.unwrap();

    let queries = vec![
        SqlQuery::new("update test_rollback_table set value = 999"),
        SqlQuery::new("insert into nonexistent_table (value) values (1)"),
    ];

    match database.batch(queries).await {
        Ok(()) => {
            let _ = database.exec(SqlQuery::new("drop table test_rollback_table")).await;
            (StatusCode::INTERNAL_SERVER_ERROR, "expected batch to fail".to_owned())
        }
        Err(_) => {
            let result = database.exec(SqlQuery::new("select value from test_rollback_table")).await.unwrap();
            let value: i64 = result.into_rows().first().unwrap().columns.first().unwrap().try_into().unwrap();
            let _ = database.exec(SqlQuery::new("drop table test_rollback_table")).await;

            if value == 42 {
                (StatusCode::OK, "rollback verified".to_owned())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("expected 0 but got {}", value))
            }
        }
    }
}

async fn test_panic_page() -> String {
    panic!("test function panic")
}

async fn test_panic_restore() -> &'static str {
    unsafe {
        PANIC_RESTORE_COUNTER+=1;
    }

    panic!("test restore after panic");
}

async fn test_panic_restore_test() -> String {
    let counter_value = unsafe { PANIC_RESTORE_COUNTER };
    format!("panic counter: {counter_value}")
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

async fn test_fetch() -> HttpBody {
    let response = fetch(
        HttpRequest::get("https://httpbin.org/get").unwrap()
    ).await.unwrap();

    response.into_body()
}

async fn test_fetch_post() -> HttpBody {
    let response = fetch(
        HttpRequest::post("https://httpbin.org/post").unwrap().with_body("test fx request body")
    ).await.unwrap();

    response.into_body()
}

async fn test_fetch_query() -> HttpBody {
    let response = fetch(
        HttpRequest::get("https://httpbin.org/get").unwrap()
            .with_query(&[("param1", "value1"), ("param2", "value2")])
    ).await.unwrap();

    response.into_body()
}

async fn test_fetch_with_header() -> HttpBody {
    fetch(
        HttpRequest::get("https://httpbin.org/headers").unwrap()
            .with_header("x-custom-header".parse().unwrap(), "custom-value".parse().unwrap())
    ).await.unwrap().into_body()
}

async fn test_fetch_body_read_all() -> String {
    let response = fetch(
        HttpRequest::get("https://httpbin.org/get").unwrap()
    ).await.unwrap();

    String::from_utf8(response.into_body().read_all().await.unwrap()).unwrap()
}

async fn test_fetch_timeout() -> &'static str {
    match fetch(HttpRequest::get("http://10.255.255.1").unwrap()).await {
        Ok(_) => "unexpected success",
        Err(FetchError::ConnectionFailed) => "connection failed",
        Err(FetchError::ConnectionTimeout) => "connection timeout",
        Err(_) => "other error",
    }
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

async fn handle_cron() -> String {
    let database = cron_database().await;
    database.exec(SqlQuery::new("insert into test_cron_state (v) values (1)")).await.unwrap();
    let result = database.exec(SqlQuery::new("select sum(v) from test_cron_state")).await.unwrap().into_rows()
        .into_iter().next().unwrap();
    match &result.columns[0] {
        SqlValue::Integer(v) => v.to_string(),
        other => panic!("unexpected result type: {other:?}"),
    }
}

async fn handle_cron_custom_endpoint_for_task() -> String {
    let database = cron_database().await;
    database.exec(SqlQuery::new("insert into test_cron_custom_endpoint_state (v) values (1)")).await.unwrap();
    let result = database.exec(SqlQuery::new("select sum(v) from test_cron_custom_endpoint_state")).await.unwrap().into_rows()
        .into_iter().next().unwrap();
    match &result.columns[0] {
        SqlValue::Integer(v) => v.to_string(),
        other => panic!("unexpected result type: {other:?}"),
    }
}

async fn read_cron_status() -> String {
    let database = cron_database().await;
    let result = database.exec(SqlQuery::new("select sum(v) from test_cron_state")).await.unwrap().into_rows()
        .into_iter().next().unwrap();
    match &result.columns[0] {
        SqlValue::Integer(v) => v.to_string(),
        other => panic!("unexpected result type: {other:?}"),
    }
}

async fn read_cron_custom_endpoint_status() -> String {
    let database = cron_database().await;
    let result = database.exec(SqlQuery::new("select sum(v) from test_cron_custom_endpoint_state")).await.unwrap().into_rows()
        .into_iter().next().unwrap();
    match &result.columns[0] {
        SqlValue::Integer(v) => format!("cron with custom endpoint: {}", v),
        other => panic!("unexpected result type: {other:?}"),
    }
}

async fn cron_database() -> SqlDatabase {
    let database = fx_sdk::sql("cron-test");
    Migrations::new()
        .with_migration(Migration::new("create table test_cron_state (v integer not null)"))
        .with_migration(Migration::new("create table test_cron_custom_endpoint_state (v integer not null)"))
        .run(&database)
        .await
        .unwrap();
    database
}

async fn rpc_simple() -> String {
    info!("sending rpc request");
    let response = fetch(HttpRequest::get("http://function-rpc.fx.local/").unwrap()).await.unwrap();
    assert!(response.status().is_success());

    format!("rpc test: {}", response.text().await)
}

async fn test_stream_sse() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = stream::iter(1..=5)
        .then(|i| async move {
            sleep(Duration::from_secs(1)).await;
            let event = Event::default().data(format!("Message {i}"));
            Ok::<_, Infallible>(event)
        });

    Sse::new(stream)
}

async fn env_simple() -> &'static str {
    let value = env::get("test-env-var").unwrap();
    assert_eq!("test value", value);
    "ok."
}

async fn env_missing_file() -> (StatusCode, String) {
    match env::get("TEST_FILE_ENV_VAR") {
        None => (StatusCode::OK, "ok.".to_owned()),
        Some(v) => (StatusCode::INTERNAL_SERVER_ERROR, format!("expected env var to NOT be set in this variable, got: {v}"))
    }
}

async fn kv_simple() -> &'static str {
    let kv = kv::Kv::new("test-namespace");

    kv.set("some-key", "hello kv!").await;

    let result = kv.get("some-key").await.unwrap();
    let result = String::from_utf8(result).unwrap();

    assert_eq!(result, "hello kv!");

    "ok."
}

async fn kv_distributed_lock() -> &'static str {
    let kv = kv::Kv::new("test-namespace");

    let lock_value = fx::random(8);
    match kv.set_nx_px("test-distributed-lock", &lock_value, true, Some(Duration::from_secs(10))).await {
        Ok(_) => {},
        Err(KvSetNxPxError::AlreadyExists) => return "already locked.\n",
    };

    sleep(Duration::from_secs(3)).await;

    kv.delex_ifeq("test-distributed-lock", lock_value).await;

    "ok.\n"
}

async fn kv_pubsub_subscribe() -> String {
    let kv = kv::Kv::new("test-namespace");

    let sum = kv.subscribe("test-channel").await
        .take(3)
        .map(|v| String::from_utf8(v).unwrap().parse::<u64>().unwrap())
        .reduce(|a, b| async move { a + b}).await
        .unwrap();

    format!("result: {sum}")
}

#[derive(Deserialize)]
struct KvPubsubPublishRequest {
    value: u64,
}

async fn kv_pubsub_publish(Json(req): Json<KvPubsubPublishRequest>) -> &'static str {
    let kv = kv::Kv::new("test-namespace");

    kv.publish("test-channel", req.value.to_string().into_bytes()).await;

    "ok.\n"
}

async fn kv_tasks_background_start() -> &'static str {
    tasks::run_in_background(async {
        sleep(Duration::from_secs(1)).await;
        kv::Kv::new("test-namespace").set("background_task_status", "done").await;
    });

    "ok.\n"
}

async fn kv_tasks_background_status() -> (StatusCode, &'static str) {
    match kv::Kv::new("test-namespace").get("background_task_status").await {
        Some(_) => (StatusCode::OK, "done."),
        None => (StatusCode::NOT_FOUND, "not done yet.")
    }
}

async fn test_limits_memory() -> String {
    let mut data: Vec<u8> = Vec::new();
    data.resize(1 * 1024 * 1024 * 1024, 0xA5); // try to make vec very large to trigger memory limits, 1GB in this case (remember that wasm32 has 4GB size limit)
    format!("large allocation worked (unexpectedly): {:?}", data.len())
}

async fn test_cpu_preemption() -> String {
    let mut data = fx::random(1024);

    // simulate very CPU heavy request (takes around 2s):
    for _ in 0..10_000_000 {
        let mut hasher = Sha256::new();
        hasher.update(&data);
        data = hasher.finalize().to_vec();
    }

    format!("result: {}", BASE64_STANDARD.encode(data))
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
