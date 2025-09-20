use {
    std::{fs, time::Instant},
    fx_runtime::{
        FxRuntime,
        kv::{SqliteStorage, BoxedStorage, WithKey, EmptyStorage},
        FunctionId,
        error::FxRuntimeError,
        FxStream,
        definition::{DefinitionProvider, FunctionDefinition, KvDefinition, SqlDefinition, RpcDefinition},
        compiler::{MemoizedCompiler, SimpleCompiler, BoxedCompiler},
    },
    tokio::join,
    futures::StreamExt,
    fx_common::FxExecutionError,
};

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

    let fx = FxRuntime::new()
        .with_code_storage(storage_code)
        .with_definition_provider(definitions)
        .with_compiler(BoxedCompiler::new(MemoizedCompiler::new(storage_compiler, BoxedCompiler::new(SimpleCompiler::new()))));
        /*.with_service(
            Service::new(ServiceId::new("test-app".to_owned()))
                .allow_fetch()
                .with_storage("test-kv".to_owned(), BoxedStorage::new(SqliteStorage::in_memory().unwrap()))
                .with_storage("test-kv-disk".to_owned(), BoxedStorage::new(SqliteStorage::new("data/test-kv-disk").unwrap()))
                .with_sql_database("app".to_owned(), database_app)
        )*/

    test_simple(&fx).await;
    test_sql_simple(&fx).await;
    test_invoke_function_non_existent(&fx).await;
    test_invoke_function_non_existent_rpc(&fx).await;
    test_invoke_function_no_module_code(&fx).await;
    test_invoke_function_panic(&fx).await;
    test_invoke_function_wrong_argument(&fx).await;
    test_async_handler_simple(&fx).await;
    test_async_concurrent(&fx).await;
    test_async_rpc(&fx).await;
    test_rpc_panic(&fx).await;
    // test_fetch(&fx).await;
    test_stream_simple(&fx).await;
    test_random(&fx).await;
    test_time(&fx).await;
    test_kv_simple(&fx).await;
    test_kv_wrong_binding_name(&fx).await;
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

async fn test_simple(fx: &FxRuntime) {
    println!("> test_simple");
    let result: u32 = fx.invoke_service(&FunctionId::new("test-app".to_owned()), "simple", 10).await.unwrap();
    assert_eq!(52, result);
}

async fn test_sql_simple(fx: &FxRuntime) {
    println!("> test_sql_simple");
    let result: u64 = fx.invoke_service(&FunctionId::new("test-app".to_owned()), "sql_simple", ()).await.unwrap();
    assert_eq!(52, result);
}

async fn test_invoke_function_non_existent(fx: &FxRuntime) {
    println!("> test_invoke_function_non_existent");
    let result = fx.invoke_service::<(), ()>(&FunctionId::new("test-non-existent".to_owned()), "simple", ()).await;
    assert_eq!(Err(FxRuntimeError::ModuleCodeNotFound), result);
}

async fn test_invoke_function_non_existent_rpc(fx: &FxRuntime) {
    println!("> test_invoke_function_non_existent_rpc");
    let result = fx.invoke_service::<(), ()>(&FunctionId::new("test-app".to_owned()), "function_non_existent", ()).await;
    assert_eq!(Err(FxRuntimeError::RpcHandlerNotDefined), result);
}

async fn test_invoke_function_no_module_code(fx: &FxRuntime) {
    println!("> test_invoke_function_no_module_code");
    let result = fx.invoke_service::<(), ()>(&FunctionId::new("test-no-module-code".to_owned()), "simple", ()).await;
    assert_eq!(Err(FxRuntimeError::ModuleCodeNotFound), result);
}

async fn test_invoke_function_panic(fx: &FxRuntime) {
    println!("> test_invoke_function_panic");
    let result = fx.invoke_service::<(), ()>(&FunctionId::new("test-app".to_owned()), "test_panic", ()).await;
    match result.err().unwrap() {
        FxRuntimeError::ServiceInternalError { reason: _ } => {},
        other => panic!("expected service internal error, got: {other:?}"),
    }
}

async fn test_invoke_function_wrong_argument(fx: &FxRuntime) {
    println!("> test_invoke_function_wrong_argument");
    let result = fx.invoke_service::<String, u32>(&FunctionId::new("test-app".to_owned()), "simple", "wrong argument".to_owned()).await.err().unwrap();
    match result {
        FxRuntimeError::ServiceExecutionError { error } => match error {
            FxExecutionError::RpcRequestRead { reason: _ } => {
                // this error is expected
            },
        },
        other => panic!("unexpected fx error: {other:?}"),
    }
}

async fn test_async_handler_simple(fx: &FxRuntime) {
    println!("> test_async_handler_simple");
    let started_at = Instant::now();
    let result = fx.invoke_service::<u64, u64>(&FunctionId::new("test-app".to_owned()), "async_simple", 42).await.unwrap();
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!(42, result);
    assert!(total_time >= 2); // async_simple is expected to sleep for 3 seconds
}

async fn test_async_concurrent(fx: &FxRuntime) {
    println!("> test_async_concurrent");
    let started_at = Instant::now();
    let result = join!(
        async {
            fx.invoke_service::<u64, u64>(&FunctionId::new("test-app".to_owned()), "async_simple", 42).await.unwrap()
        },
        async {
            fx.invoke_service::<u64, u64>(&FunctionId::new("test-app".to_owned()), "async_simple", 43).await.unwrap()
        }
    );
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!((42, 43), result);
    assert!(total_time <= 4); // async_simple is expected to sleep for 3 seconds, two requests are served concurrently
}

async fn test_async_rpc(fx: &FxRuntime) {
    println!("> test_async_rpc");
    let result = fx.invoke_service::<u64, u64>(&FunctionId::new("test-app".to_owned()), "call_rpc", 42).await.unwrap();
    assert_eq!(84, result);
}

async fn test_rpc_panic(fx: &FxRuntime) {
    println!("> test_rpc_panic");
    let result = fx.invoke_service::<(), i64>(&FunctionId::new("test-app"), "call_rpc_panic", ()).await.unwrap();
    assert_eq!(42, result);
}

async fn test_fetch(fx: &FxRuntime) {
    println!("> test_fetch");
    let result = fx.invoke_service::<(), Result<String, String>>(&FunctionId::new("test-app".to_owned()), "test_fetch", ()).await.unwrap()
        .unwrap();
    assert_eq!("hello fx!", &result);
}

async fn test_stream_simple(fx: &FxRuntime) {
    println!("> test_stream_simple");
    let stream: FxStream = fx.invoke_service::<(), FxStream>(&FunctionId::new("test-app".to_owned()), "test_stream_simple", ()).await.unwrap();
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

async fn test_random(fx: &FxRuntime) {
    println!("> test_random");
    let random_bytes_0: Vec<u8> = fx.invoke_service::<u64, Vec<u8>>(&FunctionId::new("test-app".to_owned()), "test_random", 32).await.unwrap();
    let random_bytes_1: Vec<u8> = fx.invoke_service::<u64, Vec<u8>>(&FunctionId::new("test-app".to_owned()), "test_random", 32).await.unwrap();

    assert_eq!(32, random_bytes_0.len());
    assert_eq!(32, random_bytes_1.len());
    assert!(random_bytes_0 != random_bytes_1);
}

async fn test_time(fx: &FxRuntime) {
    println!("> test_time");
    let millis = fx.invoke_service::<(), u64>(&FunctionId::new("test-app".to_owned()), "test_time", ()).await.unwrap();
    assert!((950..=1050).contains(&millis));
}

async fn test_kv_simple(fx: &FxRuntime) {
    println!("> test_kv_simple");

    let result = fx.invoke_service::<(), Option<String>>(&FunctionId::new("test-app"), "test_kv_get", ()).await.unwrap();
    assert!(result.is_none());

    fx.invoke_service::<String, ()>(&FunctionId::new("test-app"), "test_kv_set", "Hello World!".to_owned()).await.unwrap();

    let result = fx.invoke_service::<(), Option<String>>(&FunctionId::new("test-app"), "test_kv_get", ()).await.unwrap().unwrap();
    assert_eq!("Hello World!", result);
}

async fn test_kv_wrong_binding_name(fx: &FxRuntime) {
    println!("> test_kv_wrong_binding_name");
    fx.invoke_service::<(), ()>(&FunctionId::new("test-app"), "test_kv_wrong_binding_name", ()).await.unwrap();
}
