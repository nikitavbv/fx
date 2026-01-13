use {
    std::{fs, sync::Arc, time::Instant},
    once_cell::sync::Lazy,
    tokio::join,
    parking_lot::ReentrantMutex,
    futures::StreamExt,
    fx_common::FxExecutionError,
    fx_runtime::{
        FunctionId,
        FxRuntime,
        FxStream,
        compiler::{BoxedCompiler, CraneliftCompiler, MemoizedCompiler},
        definition::{DefinitionProvider, FunctionDefinition, KvDefinition, RpcDefinition, SqlDefinition},
        kv::{BoxedStorage, EmptyStorage, SqliteStorage, WithKey},
        logs::{BoxLogger, EventFieldValue, LogEventType},
        error::FxRuntimeError,
        FunctionInvokeAndExecuteError,
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

static FX_INSTANCE: Lazy<ReentrantMutex<FxRuntime>> = Lazy::new(|| ReentrantMutex::new({
    let storage_code = BoxedStorage::new(SqliteStorage::in_memory().unwrap())
        .with_key(b"test-app", &fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        // separate instance of test app for panic to avoid disrupting other tests:
        .with_key(b"test-app-for-panic", &fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        .with_key(b"test-app-system", &fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        .with_key(b"other-app", &fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap();

    let storage_compiler = BoxedStorage::new(SqliteStorage::in_memory().unwrap());

    let definitions = DefinitionProvider::new(BoxedStorage::new(EmptyStorage))
        .with_definition(
            FunctionId::new("test-app"),
            FunctionDefinition::new()
                .with_kv(KvDefinition::new("test-kv", "data/test-kv"))
                .with_sql(SqlDefinition::new("app"))
                .with_rpc(RpcDefinition::new("other-app"))
        );

    FxRuntime::new()
        .with_code_storage(storage_code)
        .with_definition_provider(definitions)
        .with_compiler(BoxedCompiler::new(MemoizedCompiler::new(storage_compiler, BoxedCompiler::new(CraneliftCompiler::new()))))
        .with_logger(BoxLogger::new(LOGGER.clone()))
}));

#[tokio::test]
async fn simple() {
    assert_eq!(52, FX_INSTANCE.lock().invoke_service::<_, u32>(&FunctionId::new("test-app".to_owned()), "simple", 10).await.unwrap().0);
}

#[tokio::test]
async fn sql_simple() {
    assert_eq!(52, FX_INSTANCE.lock().invoke_service::<_, u32>(&FunctionId::new("test-app".to_owned()), "sql_simple", ()).await.unwrap().0);
}

#[tokio::test]
async fn invoke_function_non_existent() {
    let err = FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-non-existent".to_owned()), "simple", ())
        .await
        .map(|v| v.0)
        .err()
        .unwrap();

    match err {
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn invoke_function_non_existent_rpc() {
    let err = FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-app".to_owned()), "function_non_existent", ())
        .await
        .map(|v| v.0);

    match err {
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn invoke_function_no_module_code() {
    let err = FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-no-module-code".to_owned()), "simple", ())
        .await
        .map(|v| v.0);

    match err {
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn invoke_function_panic() {
    let result = FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-app-for-panic".to_owned()), "test_panic", ()).await.map(|v| v.0);
    match result.err().unwrap() {
        other => panic!("expected service internal error, got: {other:?}"),
    }
}

#[tokio::test]
async fn invoke_function_wrong_argument() {
    let result = FX_INSTANCE.lock().invoke_service::<String, u32>(&FunctionId::new("test-app".to_owned()), "simple", "wrong argument".to_owned()).await.err().unwrap();
    match result {
        other => panic!("unexpected fx error: {other:?}"),
    }
}

#[tokio::test]
async fn async_handler_simple() {
    let started_at = Instant::now();
    let result = FX_INSTANCE.lock().invoke_service::<u64, u64>(&FunctionId::new("test-app".to_owned()), "async_simple", 42).await.unwrap().0;
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!(42, result);
    assert!(total_time >= 2); // async_simple is expected to sleep for 3 seconds
}

#[tokio::test]
async fn async_concurrent() {
    // pre-warm the function
    let fx = FX_INSTANCE.lock();
    let _ = fx.invoke_service::<u64, u64>(&FunctionId::new("test-app".to_owned()), "async_simple", 42).await.unwrap().0;

    // measure time to wait for both
    let function = FunctionId::new("test-app".to_owned());
    let started_at = Instant::now();
    let result = join!(
        async {
            fx.invoke_service::<u64, u64>(&function, "async_simple", 42).await.unwrap().0
        },
        async {
            fx.invoke_service::<u64, u64>(&function, "async_simple", 43).await.unwrap().0
        }
    );
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!((42, 43), result);
    println!("waited for {total_time}");
    assert!(total_time <= 4); // async_simple is expected to sleep for 3 seconds, two requests are served concurrentlys
}

#[tokio::test]
async fn async_rpc() {
    assert_eq!(
        84,
        FX_INSTANCE.lock().invoke_service::<u64, u64>(&FunctionId::new("test-app".to_owned()), "call_rpc", 42).await.unwrap().0
    );
}

#[tokio::test]
async fn rpc_panic() {
    assert_eq!(
        42,
        FX_INSTANCE.lock().invoke_service::<(), i64>(&FunctionId::new("test-app"), "call_rpc_panic", ()).await.unwrap().0
    )
}

#[tokio::test]
async fn stream_simple() {
    let fx = FX_INSTANCE.lock();
    let stream: FxStream = fx.invoke_service::<(), FxStream>(&FunctionId::new("test-app".to_owned()), "test_stream_simple", ()).await.unwrap().0;
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
}

#[tokio::test]
async fn random() {
    let random_bytes_0: Vec<u8> = FX_INSTANCE.lock().invoke_service::<u64, Vec<u8>>(&FunctionId::new("test-app".to_owned()), "test_random", 32).await.unwrap().0;
    let random_bytes_1: Vec<u8> = FX_INSTANCE.lock().invoke_service::<u64, Vec<u8>>(&FunctionId::new("test-app".to_owned()), "test_random", 32).await.unwrap().0;

    assert_eq!(32, random_bytes_0.len());
    assert_eq!(32, random_bytes_1.len());
    assert!(random_bytes_0 != random_bytes_1);
}

#[tokio::test]
async fn time() {
    let millis = FX_INSTANCE.lock().invoke_service::<(), u64>(&FunctionId::new("test-app".to_owned()), "test_time", ()).await.unwrap().0;
    assert!((950..=1050).contains(&millis));
}

#[tokio::test]
async fn kv_simple() {
    let _cleanup_guard = DataCleanupGuard;

    let result = FX_INSTANCE.lock().invoke_service::<(), Option<String>>(&FunctionId::new("test-app"), "test_kv_get", ()).await.unwrap().0;
    assert!(result.is_none());

    FX_INSTANCE.lock().invoke_service::<String, ()>(&FunctionId::new("test-app"), "test_kv_set", "Hello World!".to_owned()).await.unwrap();

    let result = FX_INSTANCE.lock().invoke_service::<(), Option<String>>(&FunctionId::new("test-app"), "test_kv_get", ()).await.unwrap().0.unwrap();
    assert_eq!("Hello World!", result);
}

#[tokio::test]
async fn kv_wrong_binding_name() {
    FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_kv_wrong_binding_name", ()).await.unwrap();
}

#[tokio::test]
async fn fetch() {
    let result = FX_INSTANCE.lock()
        .invoke_service::<(), Result<String, String>>(&FunctionId::new("test-app".to_owned()), "test_fetch", ()).await.unwrap().0
        .unwrap();
    assert_eq!("hello fx!", &result);
}

#[tokio::test]
async fn log() {
    FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_log", ()).await.unwrap();

    let events = LOGGER.events();
    let found_expected_event = events.iter()
        .find(|v| v.fields.get("message").unwrap() == &EventFieldValue::Text("this is a test log".to_owned()))
        .is_some();

    if !found_expected_event {
        panic!("didn't find expected event. All events: {events:?}");
    }
}

#[tokio::test]
async fn log_span() {
    FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_log_span", ()).await.unwrap();

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
async fn metrics_counter_increment() {
    FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_counter_increment", ()).await.unwrap();
    // todo: check counter value
}

#[tokio::test]
async fn metrics_counter_increment_twice_with_tags() {
    FX_INSTANCE.lock().invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_counter_increment_twice_with_tags", ()).await.unwrap();
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
