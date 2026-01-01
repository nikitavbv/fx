use {
    std::{fs, sync::Arc, time::Instant},
    fx_common::FxExecutionError,
    fx_runtime::{
        FunctionId,
        FxRuntime,
        compiler::{BoxedCompiler, CraneliftCompiler, MemoizedCompiler},
        definition::{DefinitionProvider, FunctionDefinition, KvDefinition, RpcDefinition, SqlDefinition},
        error::FxRuntimeError,
        kv::{BoxedStorage, EmptyStorage, SqliteStorage, WithKey},
        logs::BoxLogger,
    },
    once_cell::sync::Lazy,
    crate::logger::TestLogger,
};

mod logger;

static FX_INSTANCE: Lazy<FxRuntime> = Lazy::new(|| {
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

    let logger = Arc::new(TestLogger::new());

    FxRuntime::new()
        .with_code_storage(storage_code)
        .with_definition_provider(definitions)
        .with_compiler(BoxedCompiler::new(MemoizedCompiler::new(storage_compiler, BoxedCompiler::new(CraneliftCompiler::new()))))
        .with_logger(BoxLogger::new(logger.clone()))
});

#[tokio::test]
async fn simple() {
    assert_eq!(52, FX_INSTANCE.invoke_service::<_, u32>(&FunctionId::new("test-app".to_owned()), "simple", 10).await.unwrap().0);
}

#[tokio::test]
async fn sql_simple() {
    assert_eq!(52, FX_INSTANCE.invoke_service::<_, u32>(&FunctionId::new("test-app".to_owned()), "sql_simple", ()).await.unwrap().0);
}

#[tokio::test]
async fn invoke_function_non_existent() {
    assert_eq!(
        Err(FxRuntimeError::ModuleCodeNotFound),
        FX_INSTANCE.invoke_service::<(), ()>(&FunctionId::new("test-non-existent".to_owned()), "simple", ())
            .await
            .map(|v| v.0)
    )
}

#[tokio::test]
async fn invoke_function_non_existent_rpc() {
    assert_eq!(
        Err(FxRuntimeError::RpcHandlerNotDefined),
        FX_INSTANCE.invoke_service::<(), ()>(&FunctionId::new("test-app".to_owned()), "function_non_existent", ())
            .await
            .map(|v| v.0)
    )
}

#[tokio::test]
async fn invoke_function_no_module_code() {
    assert_eq!(
        Err(FxRuntimeError::ModuleCodeNotFound),
        FX_INSTANCE.invoke_service::<(), ()>(&FunctionId::new("test-no-module-code".to_owned()), "simple", ())
            .await
            .map(|v| v.0)
    )
}

#[tokio::test]
async fn invoke_function_panic() {
    let result = FX_INSTANCE.invoke_service::<(), ()>(&FunctionId::new("test-app-for-panic".to_owned()), "test_panic", ()).await.map(|v| v.0);
    match result.err().unwrap() {
        FxRuntimeError::ServiceInternalError { reason: _ } => {},
        other => panic!("expected service internal error, got: {other:?}"),
    }
}

#[tokio::test]
async fn invoke_function_wrong_argument() {
    let result = FX_INSTANCE.invoke_service::<String, u32>(&FunctionId::new("test-app".to_owned()), "simple", "wrong argument".to_owned()).await.err().unwrap();
    match result {
        FxRuntimeError::ServiceExecutionError { error } => match error {
            FxExecutionError::RpcRequestRead { reason: _ } => {
                // this error is expected
            },
        },
        other => panic!("unexpected fx error: {other:?}"),
    }
}

#[tokio::test]
async fn async_handler_simple() {
    let started_at = Instant::now();
    let result = FX_INSTANCE.invoke_service::<u64, u64>(&FunctionId::new("test-app".to_owned()), "async_simple", 42).await.unwrap().0;
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!(42, result);
    assert!(total_time >= 2); // async_simple is expected to sleep for 3 seconds
}
