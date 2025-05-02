use {
    std::{fs, time::Instant},
    fx_cloud::{FxCloud, storage::{SqliteStorage, BoxedStorage, WithKey}, sql::SqlDatabase, Service, ServiceId, error::FxCloudError},
    tokio::join,
};

#[tokio::main]
async fn main() {
    let storage_code = BoxedStorage::new(SqliteStorage::in_memory().unwrap())
        .with_key(b"test-app", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
        .with_key(b"other-app", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap());
    let storage_compiler = BoxedStorage::new(SqliteStorage::in_memory().unwrap());

    let database_cron = SqlDatabase::in_memory();
    let database_app = SqlDatabase::in_memory();

    let fx = FxCloud::new()
        .with_code_storage(storage_code)
        .with_memoized_compiler(storage_compiler)
        .with_queue()
        .with_cron(database_cron)
        .with_service(
            Service::new(ServiceId::new("test-app".to_owned()))
                .with_sql_database("app".to_owned(), database_app)
        )
        .with_service(
            Service::new(ServiceId::new("other-app".to_owned()))
        )
        .with_service(Service::new(ServiceId::new("test-no-module-code".to_owned())));

    test_simple(&fx).await;
    test_sql_simple(&fx).await;
    test_sqlx(&fx).await;
    test_invoke_function_non_existent(&fx).await;
    test_invoke_function_non_existent_rpc(&fx).await;
    test_invoke_function_no_module_code(&fx).await;
    test_async_handler_simple(&fx).await;
    test_async_concurrent(&fx).await;
    test_async_rpc(&fx).await;
    // TODO: test what happens if you invoke function with wrong argument
    // TODO: test what happens if function panics
    // TODO: test that database can only be accessed by correct binding name
    // TODO: test sql with all types
    // TODO: test sql with sqlx
    // TODO: test sql with error

    println!("all tests passed");
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
