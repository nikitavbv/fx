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

struct DataCleanupGuard;

impl Drop for DataCleanupGuard {
    fn drop(&mut self) {
        fs::remove_file("data/test-kv/test-key").unwrap();
    }
}

static LOGGER: Lazy<Arc<TestLogger>> = Lazy::new(|| Arc::new(TestLogger::new()));
static LOGGER_CUSTOM_FUNCTION: Lazy<Arc<TestLogger>> = Lazy::new(|| Arc::new(TestLogger::new()));

#[tokio::test]
async fn simple() {
    let response = fx_server()
        .invoke_function(
            FunctionId::new("test-app".to_owned()),
            FunctionRequest::from(hyper::Request::new(http_body_util::Full::new(hyper::body::Bytes::from_static("hello fx!".as_bytes()))))
        ).await;

    panic!("got response: {response:?}");
}

/*#[tokio::test]
async fn sql_simple() {
    assert_eq!(52, fx_server().await.lock().invoke_function::<_, u32>(&FunctionId::new("test-app".to_owned()), "sql_simple", ()).await.unwrap().0);
}

#[tokio::test]
async fn invoke_function_non_existent() {
    let err = fx_server().await.lock()
        .invoke_function::<(), ()>(&FunctionId::new("test-non-existent".to_owned()), "simple", ())
        .await
        .map(|v| v.0)
        .err()
        .unwrap();

    match err {
        FunctionInvokeAndExecuteError::CodeNotFound => {}
        other => panic!("unexpected error: {other:?}, expected CodeNotFound"),
    }
}

#[tokio::test]
async fn invoke_function_non_existent_rpc() {
    let err = fx_server().await.lock()
        .invoke_function::<(), ()>(&FunctionId::new("test-app".to_owned()), "function_non_existent", ())
        .await
        .map(|v| v.0);

    match err {
        Err(FunctionInvokeAndExecuteError::HandlerNotDefined) => {}
        other => panic!("unexpected error: {other:?}, expected HandlerNotDefined"),
    }
}

#[tokio::test]
async fn invoke_function_no_module_code() {
    let err = fx_server().await.lock()
        .invoke_function::<(), ()>(&FunctionId::new("test-no-module-code".to_owned()), "simple", ())
        .await
        .map(|v| v.0);

    match err {
        Err(FunctionInvokeAndExecuteError::CodeNotFound) => {}
        other => panic!("unexpected error: {other:?}, expected CodeNotFound"),
    }
}

#[tokio::test]
async fn invoke_function_panic() {
    let result = fx_server().await.lock()
        .invoke_function::<(), ()>(&FunctionId::new("test-app-for-panic".to_owned()), "test_panic", ()).await.map(|v| v.0);
    match result.err().unwrap() {
        FunctionInvokeAndExecuteError::FunctionPanicked => {}
        other => panic!("expected function panicked error, got: {other:?}"),
    }
}

#[tokio::test]
async fn invoke_function_wrong_argument() {
    let result = fx_server().await.lock()
        .invoke_function::<String, u32>(&FunctionId::new("test-app".to_owned()), "simple", "wrong argument".to_owned()).await.err().unwrap();
    match result {
        FunctionInvokeAndExecuteError::FunctionRuntimeError => {}
        other => panic!("unexpected fx error: {other:?}"),
    }
}

#[tokio::test]
async fn async_handler_simple() {
    let started_at = Instant::now();
    let result = fx_server().await.lock()
        .invoke_function::<u64, u64>(&FunctionId::new("test-app".to_owned()), "async_simple", 42).await.unwrap().0;
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!(42, result);
    assert!(total_time >= 2); // async_simple is expected to sleep for 3 seconds
}

#[tokio::test]
async fn async_concurrent() {
    // pre-warm the function
    let fx = fx_server().await;
    let fx = fx.lock();
    let _ = fx.invoke_function::<u64, u64>(&FunctionId::new("test-app".to_owned()), "async_simple", 42).await.unwrap().0;

    // measure time to wait for both
    let function = FunctionId::new("test-app".to_owned());
    let started_at = Instant::now();
    let result = join!(
        async {
            fx.invoke_function::<u64, u64>(&function, "async_simple", 42).await.unwrap().0
        },
        async {
            fx.invoke_function::<u64, u64>(&function, "async_simple", 43).await.unwrap().0
        }
    );
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!((42, 43), result);
    println!("waited for {total_time}");
    assert!(total_time <= 4); // async_simple is expected to sleep for 3 seconds, two requests are served concurrentlys
}

/*#[tokio::test]
async fn stream_simple() {
    let fx = fx_server().await.lock();
    let stream: FxStream = fx.invoke_function::<(), FxStream>(&FunctionId::new("test-app".to_owned()), "test_stream_simple", ()).await.unwrap().0;
    let mut stream = fx.read_stream(&stream).unwrap().unwrap();
    let started_at = Instant::now();
    let mut n = 0;
    while let Some(v) = stream.next().await {
        let v = v.unwrap();
        if n != v[0] || v.len() > 1 {
            panic!("recieved unexpected data in stream: {v:?}");
        }

        let millis_passed = (Instant::now() - started_at).as_millis();
        if !(millis_passed >= (n as u128) * 1000 && millis_passed < (n as u128 + 1) * 1000) {
            panic!("unexpected amount of time passed: {millis_passed}");
        }

        n += 1;
    }

    if n != 5 {
        panic!("unexpected number of items read from stream: {n}");
    }
}*/

#[tokio::test]
async fn random() {
    let random_bytes_0: Vec<u8> = fx_server().await.lock().invoke_function::<u64, Vec<u8>>(&FunctionId::new("test-app".to_owned()), "test_random", 32).await.unwrap().0;
    let random_bytes_1: Vec<u8> = fx_server().await.lock().invoke_function::<u64, Vec<u8>>(&FunctionId::new("test-app".to_owned()), "test_random", 32).await.unwrap().0;

    assert_eq!(32, random_bytes_0.len());
    assert_eq!(32, random_bytes_1.len());
    assert!(random_bytes_0 != random_bytes_1);
}

#[tokio::test]
async fn time() {
    let millis = fx_server().await.lock().invoke_function::<(), u64>(&FunctionId::new("test-app".to_owned()), "test_time", ()).await.unwrap().0;
    assert!((950..=1050).contains(&millis));
}

#[tokio::test]
async fn kv_simple() {
    let _cleanup_guard = DataCleanupGuard;

    let result = fx_server().await.lock().invoke_function::<(), Option<String>>(&FunctionId::new("test-app"), "test_kv_get", ()).await.unwrap().0;
    assert!(result.is_none());

    fx_server().await.lock().invoke_function::<String, ()>(&FunctionId::new("test-app"), "test_kv_set", "Hello World!".to_owned()).await.unwrap();

    let result = fx_server().await.lock().invoke_function::<(), Option<String>>(&FunctionId::new("test-app"), "test_kv_get", ()).await.unwrap().0.unwrap();
    assert_eq!("Hello World!", result);
}

#[tokio::test]
async fn kv_wrong_binding_name() {
    fx_server().await.lock().invoke_function::<(), ()>(&FunctionId::new("test-app"), "test_kv_wrong_binding_name", ()).await.unwrap();
}

/*#[tokio::test]
async fn fetch() {
    let result = fx_server().await.lock()
        .invoke_function::<(), Result<String, String>>(&FunctionId::new("test-app".to_owned()), "test_fetch", ()).await.unwrap().0
        .unwrap();
    assert_eq!("hello fx!", &result);
}*/

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

fn fx_server() -> &'static RunningFxServer {
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

            tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
                server.deploy_function(
                    FunctionId::new("test-app"),
                    FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
                        .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
                        .with_binding_kv("test-kv".to_owned(), current_dir().unwrap().join("data/test-kv").to_str().unwrap().to_string())
                        .with_binding_sql("app".to_owned(), ":memory:".to_owned())
                        .with_binding_rpc("other-app".to_owned(), "/tmp/fx/functions/other-app.fx.yaml".to_owned())
                ).await;
            });

            server
        })).join().unwrap()
    })

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
