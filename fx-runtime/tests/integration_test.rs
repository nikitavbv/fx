use {
    std::{fs, sync::Arc},
    once_cell::sync::Lazy,
    fx_runtime::{
        FxRuntime,
        kv::{BoxedStorage, SqliteStorage, WithKey, EmptyStorage},
        FunctionId,
        definition::{DefinitionProvider, FunctionDefinition, KvDefinition, SqlDefinition, RpcDefinition},
        compiler::{BoxedCompiler, MemoizedCompiler, CraneliftCompiler},
        logs::BoxLogger,
    },
    crate::logger::TestLogger,
};

mod logger;

static FX_INSTANCE: Lazy<FxRuntime> = Lazy::new(|| {
    let storage_code = BoxedStorage::new(SqliteStorage::in_memory().unwrap())
        .with_key(b"test-app", &fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
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
