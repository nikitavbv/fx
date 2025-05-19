use {
    std::{fs, time::{Instant, Duration}},
    fx_cloud::{FxCloud, storage::{SqliteStorage, BoxedStorage, WithKey}, sql::SqlDatabase, Service, ServiceId, error::FxCloudError, QUEUE_SYSTEM_INVOCATIONS, FxStream},
    tokio::{join, time::sleep},
    futures::StreamExt,
    fx_core::FxExecutionError,
};

#[tokio::main]
async fn main() {
    let started_at = Instant::now();

    if !fs::exists("data").unwrap() {
        fs::create_dir("data").unwrap();
    }

    let storage_code = BoxedStorage::new(SqliteStorage::in_memory().unwrap())
        .with_key(b"test-app", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        .with_key(b"test-app-global", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        .with_key(b"test-app-system", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        .with_key(b"test-invocation-count", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap()
        .with_key(b"other-app", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap()).unwrap();
    let storage_compiler = BoxedStorage::new(SqliteStorage::in_memory().unwrap());

    let database_cron = SqlDatabase::in_memory().unwrap();
    let database_app = SqlDatabase::in_memory().unwrap();

    let fx = FxCloud::new()
        .with_code_storage(storage_code)
        .with_memoized_compiler(storage_compiler)
        .with_queue().await
        .with_cron(database_cron).unwrap()
        .with_service(
            Service::new(ServiceId::new("test-app".to_owned()))
                .allow_fetch()
                .with_storage("test-kv".to_owned(), BoxedStorage::new(SqliteStorage::in_memory().unwrap()))
                .with_storage("test-kv-disk".to_owned(), BoxedStorage::new(SqliteStorage::new("data/test-kv-disk").unwrap()))
                .with_sql_database("app".to_owned(), database_app)
        )
        .with_service(Service::new(ServiceId::new("test-app-global".to_owned())).global())
        .with_service(Service::new(ServiceId::new("test-app-system".to_owned())).global().system())
        .with_service(Service::new(ServiceId::new("other-app".to_owned())))
        .with_service(Service::new(ServiceId::new("test-no-module-code".to_owned())))
        .with_service(Service::new(ServiceId::new("test-invocation-count".to_owned())))
        .with_queue_subscription(QUEUE_SYSTEM_INVOCATIONS, ServiceId::new("test-app-system".to_owned()), "on_invoke").await;

    fx.run_queue().await;

    test_simple(&fx).await;
    test_sql_simple(&fx).await;
    test_sqlx(&fx).await;
    test_invoke_function_non_existent(&fx).await;
    test_invoke_function_non_existent_rpc(&fx).await;
    test_invoke_function_no_module_code(&fx).await;
    test_invoke_function_panic(&fx).await;
    test_invoke_function_wrong_argument(&fx).await;
    test_async_handler_simple(&fx).await;
    test_async_concurrent(&fx).await;
    test_async_rpc(&fx).await;
    test_rpc_panic(&fx).await;
    test_fetch(&fx).await;
    test_global(&fx).await;
    test_queue_system_invocations(&fx).await;
    test_stream_simple(&fx).await;
    test_random(&fx).await;
    test_time(&fx).await;
    test_kv_simple(&fx).await;
    test_kv_wrong_binding_name(&fx).await;
    test_kv_disk(&fx).await;
    // TODO: sql transactions
    // TODO: test that database can only be accessed by correct binding name
    // TODO: test sql with all types
    // TODO: test sql with sqlx
    // TODO: test sql with error
    // TODO: test a lot of async calls in a loop with random response times to verify that multiple concurrent requests are handled correctly
    // TODO: test what happens if function responds with incorrect type

    println!("all tests passed in {:?}", Instant::now() - started_at);
}

async fn test_simple(fx: &FxCloud) {
    println!("> test_simple");
    let result: u32 = fx.invoke_service(&ServiceId::new("test-app".to_owned()), "simple", 10).await.unwrap();
    assert_eq!(52, result);
}

async fn test_sql_simple(fx: &FxCloud) {
    println!("> test_sql_simple");
    let result: u64 = fx.invoke_service(&ServiceId::new("test-app".to_owned()), "sql_simple", ()).await.unwrap();
    assert_eq!(52, result);
}

async fn test_sqlx(fx: &FxCloud) {
    println!("> test_sqlx");
    let result: u64 = fx.invoke_service(&ServiceId::new("test-app".to_owned()), "sqlx", ()).await.unwrap();
    assert_eq!(52, result);
}

async fn test_invoke_function_non_existent(fx: &FxCloud) {
    println!("> test_invoke_function_non_existent");
    let result = fx.invoke_service::<(), ()>(&ServiceId::new("test-non-existent".to_owned()), "simple", ()).await;
    assert_eq!(Err(FxCloudError::ServiceNotFound), result);
}

async fn test_invoke_function_non_existent_rpc(fx: &FxCloud) {
    println!("> test_invoke_function_non_existent_rpc");
    let result = fx.invoke_service::<(), ()>(&ServiceId::new("test-app".to_owned()), "function_non_existent", ()).await;
    assert_eq!(Err(FxCloudError::RpcHandlerNotDefined), result);
}

async fn test_invoke_function_no_module_code(fx: &FxCloud) {
    println!("> test_invoke_function_no_module_code");
    let result = fx.invoke_service::<(), ()>(&ServiceId::new("test-no-module-code".to_owned()), "simple", ()).await;
    assert_eq!(Err(FxCloudError::ModuleCodeNotFound), result);
}

async fn test_invoke_function_panic(fx: &FxCloud) {
    println!("> test_invoke_function_panic");
    let result = fx.invoke_service::<(), ()>(&ServiceId::new("test-app".to_owned()), "test_panic", ()).await;
    match result.err().unwrap() {
        FxCloudError::ServiceInternalError { reason: _ } => {},
        other => panic!("expected service internal error, got: {other:?}"),
    }
}

async fn test_invoke_function_wrong_argument(fx: &FxCloud) {
    println!("> test_invoke_function_wrong_argument");
    let result = fx.invoke_service::<String, u32>(&ServiceId::new("test-app".to_owned()), "simple", "wrong argument".to_owned()).await.err().unwrap();
    match result {
        FxCloudError::ServiceExecutionError { error } => match error {
            FxExecutionError::RpcRequestRead { reason: _ } => assert!(true),
        },
        other => panic!("unexpected fx error: {other:?}"),
    }
}

async fn test_async_handler_simple(fx: &FxCloud) {
    println!("> test_async_handler_simple");
    let started_at = Instant::now();
    let result = fx.invoke_service::<u64, u64>(&ServiceId::new("test-app".to_owned()), "async_simple", 42).await.unwrap();
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!(42, result);
    assert!(total_time >= 2); // async_simple is expected to sleep for 3 seconds
}

async fn test_async_concurrent(fx: &FxCloud) {
    println!("> test_async_concurrent");
    let started_at = Instant::now();
    let result = join!(
        async {
            fx.invoke_service::<u64, u64>(&ServiceId::new("test-app".to_owned()), "async_simple", 42).await.unwrap()
        },
        async {
            fx.invoke_service::<u64, u64>(&ServiceId::new("test-app".to_owned()), "async_simple", 43).await.unwrap()
        }
    );
    let total_time = (Instant::now() - started_at).as_secs();
    assert_eq!((42, 43), result);
    assert!(total_time <= 4); // async_simple is expected to sleep for 3 seconds, two requests are served concurrently
}

async fn test_async_rpc(fx: &FxCloud) {
    println!("> test_async_rpc");
    let result = fx.invoke_service::<u64, u64>(&ServiceId::new("test-app".to_owned()), "call_rpc", 42).await.unwrap();
    assert_eq!(84, result);
}

async fn test_rpc_panic(fx: &FxCloud) {
    println!("> test_rpc_panic");
    let result = fx.invoke_service::<(), i64>(&ServiceId::new("test-app"), "call_rpc_panic", ()).await.unwrap();
    assert_eq!(42, result);
}

async fn test_fetch(fx: &FxCloud) {
    println!("> test_fetch");
    let result = fx.invoke_service::<(), Result<String, String>>(&ServiceId::new("test-app".to_owned()), "test_fetch", ()).await.unwrap()
        .unwrap();
    assert_eq!("hello fx!", &result);
}

async fn test_global(fx: &FxCloud) {
    println!("> test_global");
    let result1 = fx.invoke_service::<(), u64>(&ServiceId::new("test-app-global".to_owned()), "global_counter_inc", ()).await.unwrap();
    let result2 = fx.invoke_service::<(), u64>(&ServiceId::new("test-app-global".to_owned()), "global_counter_inc", ()).await.unwrap();
    assert!(result2 > result1);
}

async fn test_queue_system_invocations(fx: &FxCloud) {
    println!("> test_queue_system_invocations");
    let before = fx.invoke_service::<String, u64>(&ServiceId::new("test-app-system".to_owned()), "get_invoke_count", "test-invocation-count".to_owned()).await.unwrap();
    assert_eq!(0, before);

    fx.invoke_service::<u32, u32>(&ServiceId::new("test-invocation-count".to_owned()), "simple", 10).await.unwrap();

    let mut after = 0;
    for _retry in 0..20 {
        after = fx.invoke_service::<String, u64>(&ServiceId::new("test-app-system".to_owned()), "get_invoke_count", "test-invocation-count".to_owned()).await.unwrap();
        if after == 1 {
            break;
        } else {
            // queues are processed async, so may have to wait a bit
            sleep(Duration::from_millis(100)).await;
        }
    }
    assert_eq!(1, after);
}

async fn test_stream_simple(fx: &FxCloud) {
    println!("> test_stream_simple");
    let stream: FxStream = fx.invoke_service::<(), FxStream>(&ServiceId::new("test-app".to_owned()), "test_stream_simple", ()).await.unwrap();
    let mut stream = fx.read_stream(&stream);
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

async fn test_random(fx: &FxCloud) {
    println!("> test_random");
    let random_bytes_0: Vec<u8> = fx.invoke_service::<u64, Vec<u8>>(&ServiceId::new("test-app".to_owned()), "test_random", 32).await.unwrap();
    let random_bytes_1: Vec<u8> = fx.invoke_service::<u64, Vec<u8>>(&ServiceId::new("test-app".to_owned()), "test_random", 32).await.unwrap();

    assert_eq!(32, random_bytes_0.len());
    assert_eq!(32, random_bytes_1.len());
    assert!(random_bytes_0 != random_bytes_1);
}

async fn test_time(fx: &FxCloud) {
    println!("> test_time");
    let millis = fx.invoke_service::<(), u64>(&ServiceId::new("test-app".to_owned()), "test_time", ()).await.unwrap();
    assert!(millis >= 950 && millis <= 1050);
}

async fn test_kv_simple(fx: &FxCloud) {
    println!("> test_kv_simple");

    let result = fx.invoke_service::<(), Option<String>>(&ServiceId::new("test-app"), "test_kv_get", ()).await.unwrap();
    assert!(result.is_none());

    fx.invoke_service::<String, ()>(&ServiceId::new("test-app"), "test_kv_set", "Hello World!".to_owned()).await.unwrap();

    let result = fx.invoke_service::<(), Option<String>>(&ServiceId::new("test-app"), "test_kv_get", ()).await.unwrap().unwrap();
    assert_eq!("Hello World!", result);
}

async fn test_kv_wrong_binding_name(fx: &FxCloud) {
    println!("> test_kv_wrong_binding_name");
    fx.invoke_service::<(), ()>(&ServiceId::new("test-app"), "test_kv_wrong_binding_name", ()).await.unwrap();
}

async fn test_kv_disk(fx: &FxCloud) {
    println!("> test_kv_disk");
    fx.invoke_service::<(), ()>(&ServiceId::new("test-app"), "test_kv_disk", ()).await.unwrap();
}
