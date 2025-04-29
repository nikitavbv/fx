use {
    std::fs,
    fx_cloud::{FxCloud, storage::{SqliteStorage, BoxedStorage, WithKey}, sql::SqlDatabase, Service, ServiceId},
};

fn main() {
    let storage_code = BoxedStorage::new(SqliteStorage::in_memory().unwrap())
        .with_key(b"test-app", &fs::read("./target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap());
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
        );

    test_simple(&fx);
    test_sql_simple(&fx);
    test_sqlx(&fx);
    // TODO: test what happens if you invoke function that does not exist
    // TODO: test what happens if you invoke function with wrong argument
    // TODO: test what happens if function panics
    // TODO: test that database can only be accessed by correct binding name
    // TODO: test sql with all types
    // TODO: test sql with sqlx
    // TODO: test sql with error

    println!("all tests passed");
}

fn test_simple(fx: &FxCloud) {
    println!("> test_simple");
    let result: u32 = fx.invoke_service(&ServiceId::new("test-app".to_owned()), "simple", 10).unwrap();
    assert_eq!(52, result);
}

fn test_sql_simple(fx: &FxCloud) {
    println!("> test_sql_simple");
    let result: u64 = fx.invoke_service(&ServiceId::new("test-app".to_owned()), "sql_simple", ()).unwrap();
    assert_eq!(52, result);
}

fn test_sqlx(fx: &FxCloud) {
    println!("> test_sqlx");
    let result: u64 = fx.invoke_service(&ServiceId::new("test-app".to_owned()), "sqlx", ()).unwrap();
    assert_eq!(52, result);
}
