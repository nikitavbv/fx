use {
    std::{fs, sync::{Arc, OnceLock}, time::Instant, env::current_dir},
    once_cell::sync::Lazy,
    tokio::{join, sync::{OnceCell, Mutex}},
    parking_lot::ReentrantMutex,
    futures::StreamExt,
    fx_common::FxExecutionError,
    fx_runtime::{
        runtime::{
            FunctionId,
            FxRuntime,
            FxStream,
            definition::{DefinitionProvider, FunctionDefinition, KvDefinition, RpcDefinition, SqlDefinition},
            kv::{BoxedStorage, EmptyStorage, SqliteStorage, WithKey},
            logs::{BoxLogger, EventFieldValue, LogEventType},
            error::FxRuntimeError,
            FunctionInvokeAndExecuteError,
        },
        server::{
            server::FxServer,
            config::{ServerConfig, FunctionConfig, LoggerConfig, IntrospectionConfig},
        },
        v2::{FxServerV2, RunningFxServer, FunctionRequest},
    },
    crate::logger::TestLogger,
};

mod logger;

static LOGGER: Lazy<Arc<TestLogger>> = Lazy::new(|| Arc::new(TestLogger::new()));
static LOGGER_CUSTOM_FUNCTION: Lazy<Arc<TestLogger>> = Lazy::new(|| Arc::new(TestLogger::new()));

#[tokio::test]
async fn simple() {
    init_fx_server();
    let response = reqwest::get("http://localhost:8080").await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("hello fx!", response.text().await.unwrap());
}

#[tokio::test]
async fn status_code() {
    init_fx_server();
    let response = reqwest::get("http://localhost:8080/test/status-code").await.unwrap();
    assert_eq!(418, response.status().as_u16());
}

#[tokio::test]
async fn sql_simple() {
    init_fx_server();
    let response = reqwest::get("http://localhost:8080/test/sql-simple").await.unwrap();
    assert_eq!("52", response.text().await.unwrap());
}

// TODO: recover from panics?
#[tokio::test]
async fn function_panic() {
    init_fx_server();
    let response = reqwest::get("http://localhost:8080/test/panic").await.unwrap();
    assert_eq!(502, response.status().as_u16());
    assert_eq!("function panicked while handling request.\n", response.text().await.unwrap());
}

#[tokio::test]
async fn async_simple() {
    init_fx_server();

    let started_at = Instant::now();
    let response = reqwest::get("http://localhost:8080/test/sleep").await.unwrap();
    let total_time = (Instant::now() - started_at).as_secs();

    assert!(response.status().is_success());
    assert!(total_time >= 2);
    assert!(total_time <= 4);
}

#[tokio::test]
async fn async_conurrent() {
    init_fx_server();

    let started_at = Instant::now();
    let _ = join!(
        async {
            let response = reqwest::get("http://localhost:8080/test/sleep").await.unwrap();
            assert!(response.status().is_success());
        },
        async {
            let response = reqwest::get("http://localhost:8080/test/sleep").await.unwrap();
            assert!(response.status().is_success());
        },
    );
    let total_time = (Instant::now() - started_at).as_secs();
    assert!(total_time >= 2);
    assert!(total_time <= 4);
}

#[tokio::test]
async fn test_random() {
    init_fx_server();

    let response1 = reqwest::get("http://localhost:8080/test/random").await.unwrap().text().await.unwrap();
    let response2 = reqwest::get("http://localhost:8080/test/random").await.unwrap().text().await.unwrap();
    assert!(response1 != response2);
    assert!((30..50).contains(&response1.len()));
    assert!((30..50).contains(&response2.len()));
}

#[tokio::test]
async fn test_time() {
    init_fx_server();

    let millis: u64 = reqwest::get("http://localhost:8080/test/time").await.unwrap()
        .text().await.unwrap()
        .parse().unwrap();
    assert!((950..=1050).contains(&millis));
}

#[tokio::test]
async fn blob_simple() {
    init_fx_server();

    let client = reqwest::Client::new();

    let result = client.delete("http://localhost:8080/test/blob").send().await.unwrap();
    assert!(result.status().is_success());

    let result = client.get("http://localhost:8080/test/blob").send().await.unwrap();
    assert_eq!(404, result.status().as_u16());

    let result = client.post("http://localhost:8080/test/blob").send().await.unwrap();
    assert_eq!(200, result.status().as_u16());

    let result = client.get("http://localhost:8080/test/blob").send().await.unwrap();
    assert!(result.status().is_success());
    assert_eq!("test-value", result.text().await.unwrap());

    let result = client.delete("http://localhost:8080/test/blob").send().await.unwrap();
    assert!(result.status().is_success());
}

#[tokio::test]
async fn blob_wrong_binding_name() {
    init_fx_server();

    let result = reqwest::get("http://localhost:8080/test/blob/wrong-binding-name").await.unwrap();
    assert!(result.status().is_success());
}

#[tokio::test]
async fn fetch() {
    init_fx_server();

    let result = reqwest::get("http://localhost:8080/test/fetch").await.unwrap();
    assert!(result.status().is_success());
    assert!(result.text().await.unwrap().contains("httpbin.org/get"));
}

/*
#[tokio::test]
async fn log() {
    fx_server().await.lock().invoke_function::<(), ()>(&FunctionId::new("test-app"), "test_log", ()).await.unwrap();

    let events = LOGGER.events();
    let found_expected_event = events.iter()
        .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("this is a test log".to_owned())).unwrap_or(false))
        .is_some();

    if !found_expected_event {
        panic!("didn't find expected event. All events: {events:?}");
    }
}

#[tokio::test]
async fn log_span() {
    fx_server().await.lock().invoke_function::<(), ()>(&FunctionId::new("test-app"), "test_log_span", ()).await.unwrap();

    // both events include fields inherited from span
    let first_message = LOGGER.events()
        .into_iter()
        .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("first message".to_owned())).unwrap_or(false))
        .expect("expected first message to be present");
    let second_message = LOGGER.events()
        .into_iter()
        .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("second message".to_owned())).unwrap_or(false))
        .expect("expected second message to be present");
    assert!(first_message.fields.get("request_id").unwrap() == &EventFieldValue::Text("some-request-id".to_owned()));
    assert!(second_message.fields.get("request_id").unwrap() == &EventFieldValue::Text("some-request-id".to_owned()));

    // span begin and end are logged
    assert!(
        LOGGER.events()
            .into_iter()
            .find(|v| v.event_type == LogEventType::Begin && v.fields.get("name").map(|v| v == &EventFieldValue::Text("test_log_span".to_owned())).unwrap_or(false))
            .is_some()
    );
    assert!(
        LOGGER.events()
            .into_iter()
            .find(|v| v.event_type == LogEventType::End && v.fields.get("name").map(|v| v == &EventFieldValue::Text("test_log_span".to_owned())).unwrap_or(false))
            .is_some()
    );
}

#[tokio::test]
async fn logger_override() {
    fx_server().await.lock().invoke_function::<(), ()>(&FunctionId::new("test-app-logger-override"), "test_log", ()).await.unwrap();

    let events = LOGGER_CUSTOM_FUNCTION.events();
    let found_expected_event = events.iter()
        .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("this is a test log".to_owned())).unwrap_or(false))
        .is_some();

    if !found_expected_event {
        panic!("didn't find expected event. All events: {events:?}");
    }
}

#[tokio::test]
async fn metrics_counter_increment() {
    fx_server().await.lock().invoke_function::<(), ()>(&FunctionId::new("test-app"), "test_counter_increment", ()).await.unwrap();
    // todo: check counter value
}

#[tokio::test]
async fn metrics_counter_increment_twice_with_tags() {
    fx_server().invoke_function(FunctionId::new("test-app"), "test_counter_increment_twice_with_tags", ()).await.unwrap();
}*/

fn init_fx_server() {
    static FX_SERVER: OnceLock<RunningFxServer> = OnceLock::new();
    FX_SERVER.get_or_init(|| {
        std::thread::spawn(|| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let server = FxServerV2::new(ServerConfig {
                config_path: Some("/tmp/fx".into()),

                functions_dir: "/tmp/fx/functions".to_owned(),
                cron_data_path: None,

                logger: Some(LoggerConfig::Custom(Arc::new(BoxLogger::new(LOGGER.clone())))),

                introspection: None,
            }).start();

            server.deploy_function(
                FunctionId::new("test-app"),
                FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
                    .with_trigger_http()
                    .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
                    .with_binding_blob("test-blob".to_owned(), "/tmp/fx-test/test-blob".to_owned())
                    .with_binding_kv("test-kv".to_owned(), current_dir().unwrap().join("data/test-kv").to_str().unwrap().to_string())
                    .with_binding_sql("app".to_owned(), ":memory:".to_owned())
                    .with_binding_rpc("other-app".to_owned(), "/tmp/fx/functions/other-app.fx.yaml".to_owned())
            ).await;

            server
        })).join().unwrap()
    });

    /*static FX_SERVER: Mutex<Option<Arc<ReentrantMutex<FxServer>>>> = Mutex::const_new(None);

    let mut fx_server = FX_SERVER.lock().await;
    if let Some(fx_server) = fx_server.as_ref() {
        return fx_server.clone();
    }

    let server = FxServer::new(
        ServerConfig {
            config_path: Some("/tmp/fx".into()),

            functions_dir: "/tmp/fx/functions".to_owned(),
            cron_data_path: None,

            logger: Some(LoggerConfig::Custom(Arc::new(BoxLogger::new(LOGGER.clone())))),

            introspection: None,
        },
        FxRuntime::new()
    ).await;

    server.define_function(
        FunctionId::new("test-app-logger-override"),
        FunctionConfig::new("/tmp/fx/functions/test-app-logger-override.fx.yaml".into())
            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
            .with_logger(LoggerConfig::Custom(Arc::new(BoxLogger::new(LOGGER_CUSTOM_FUNCTION.clone()))))
    ).await;

    for function_id in ["test-app-for-panic", "test-app-for-system", "other-app"] {
        server.define_function(
            FunctionId::new(function_id),
            FunctionConfig::new("/tmp/fx/functions/test-app-for-panic.fx.yaml".into())
                .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
        ).await;
    }

    let server = Arc::new(ReentrantMutex::new(server));
    *fx_server = Some(server.clone());

    server*/
}

// TODO: add test that verifies that counter metrics with labels are recorded correctly
// TODO: sql transactions
// TODO: test that database can only be accessed by correct binding name
// TODO: test sql with all types
// TODO: test sql with sqlx
// TODO: test sql with error
// TODO: test a lot of async calls in a loop with random response times to verify that multiple concurrent requests are handled correctly
// TODO: test what happens if function responds with incorrect type
// TODO: test compiler error
// TODO: test request bodies
