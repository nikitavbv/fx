use {
    std::fs,
    fx_cloud::{FxCloud, storage::{SqliteStorage, BoxedStorage, WithKey}, sql::SqlDatabase, Service, ServiceId},
};

fn main() {
    let storage_code = BoxedStorage::new(SqliteStorage::in_memory().unwrap())
        .with_key(b"test-app/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap());
    let storage_compiler = BoxedStorage::new(SqliteStorage::in_memory().unwrap());

    let database_cron = SqlDatabase::in_memory();

    let fx = FxCloud::new()
        .with_code_storage(storage_code)
        .with_memoized_compiler(storage_compiler)
        .with_queue()
        .with_cron(database_cron)
        .with_service(Service::new(ServiceId::new("test-app".to_owned())));

    test_simple(&fx);

    println!("all tests passed");
}

fn test_simple(fx: &FxCloud) {
    let result: u32 = fx.invoke_service(&ServiceId::new("test-app".to_owned()), "simple", 10).unwrap();
    assert_eq!(52, result);
}
