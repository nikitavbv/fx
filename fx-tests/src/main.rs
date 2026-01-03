use {
    std::{fs, time::Instant, sync::Arc},
    fx_runtime::{
        FxRuntime,
        kv::{SqliteStorage, BoxedStorage, WithKey, EmptyStorage},
        FunctionId,
        error::FxRuntimeError,
        FxStream,
        definition::{DefinitionProvider, FunctionDefinition, KvDefinition, SqlDefinition, RpcDefinition},
        compiler::{MemoizedCompiler, CraneliftCompiler, BoxedCompiler},
        logs::{BoxLogger, EventFieldValue, LogEventType},
    },
    tokio::join,
    futures::StreamExt,
    fx_common::FxExecutionError,
    crate::logger::TestLogger,
};

mod logger;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let started_at = Instant::now();

    if fs::exists("data").unwrap() {
        // cleanup from previous test runs
        let _ = fs::remove_file("data/test-kv/test-key");
    } else {
        fs::create_dir("data").unwrap();
    }

    let storage_code = BoxedStorage::new(SqliteStorage::in_memory().unwrap())
        .with_key(b"test-app", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        .with_key(b"test-app-system", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        .with_key(b"other-app", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap();

    let definitions = DefinitionProvider::new(BoxedStorage::new(EmptyStorage))
        .with_definition(
            FunctionId::new("test-app"),
            FunctionDefinition::new()
                .with_kv(KvDefinition::new("test-kv", "data/test-kv"))
                .with_sql(SqlDefinition::new("app"))
                .with_rpc(RpcDefinition::new("other-app"))
        );

    let storage_compiler = BoxedStorage::new(SqliteStorage::in_memory().unwrap());

    let logger = Arc::new(TestLogger::new());

    let fx = FxRuntime::new()
        .with_code_storage(storage_code)
        .with_definition_provider(definitions)
        .with_compiler(BoxedCompiler::new(MemoizedCompiler::new(storage_compiler, BoxedCompiler::new(CraneliftCompiler::new()))))
        .with_logger(BoxLogger::new(logger.clone()));

    test_log(&fx, logger.clone()).await;
    test_log_span(&fx, logger.clone()).await;
    test_metrics_counter_increment(&fx).await;
    // TODO: add test that verifies that counter metrics with labels are recorded correctly
    test_counter_increment_twice_with_tags(&fx).await;

    // TODO: sql transactions
    // TODO: test that database can only be accessed by correct binding name
    // TODO: test sql with all types
    // TODO: test sql with sqlx
    // TODO: test sql with error
    // TODO: test a lot of async calls in a loop with random response times to verify that multiple concurrent requests are handled correctly
    // TODO: test what happens if function responds with incorrect type
    // TODO: test compiler error

    println!("all tests passed in {:?}", Instant::now() - started_at);
}

async fn test_log(fx: &FxRuntime, logger: Arc<TestLogger>) {
    println!("> test_log");
    fx.invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_log", ()).await.unwrap();

    assert!(
        logger.events()
            .into_iter()
            .find(|v| v.fields.get("message").unwrap() == &EventFieldValue::Text("this is a test log".to_owned()))
            .is_some()
    );
}

async fn test_log_span(fx: &FxRuntime, logger: Arc<TestLogger>) {
    println!("> test_log_span");
    fx.invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_log_span", ()).await.unwrap();

    // both events include fields inherited from span
    let first_message = logger.events()
        .into_iter()
        .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("first message".to_owned())).unwrap_or(false))
        .expect("expected first message to be present");
    let second_message = logger.events()
        .into_iter()
        .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("second message".to_owned())).unwrap_or(false))
        .expect("expected second message to be present");
    assert!(first_message.fields.get("request_id").unwrap() == &EventFieldValue::Text("some-request-id".to_owned()));
    assert!(second_message.fields.get("request_id").unwrap() == &EventFieldValue::Text("some-request-id".to_owned()));

    // span begin and end are logged
    assert!(
        logger.events()
            .into_iter()
            .find(|v| v.event_type == LogEventType::Begin && v.fields.get("name").map(|v| v == &EventFieldValue::Text("test_log_span".to_owned())).unwrap_or(false))
            .is_some()
    );
    assert!(
        logger.events()
            .into_iter()
            .find(|v| v.event_type == LogEventType::End && v.fields.get("name").map(|v| v == &EventFieldValue::Text("test_log_span".to_owned())).unwrap_or(false))
            .is_some()
    );
}

async fn test_metrics_counter_increment(fx: &FxRuntime) {
    println!("> test_metrics_counter_increment");
    fx.invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_counter_increment", ()).await.unwrap();
    // TODO: check counter value
}

async fn test_counter_increment_twice_with_tags(fx: &FxRuntime) {
    println!("> test_counter_increment_twice_with_tags");
    fx.invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_counter_increment_twice_with_tags", ()).await.unwrap();
}
