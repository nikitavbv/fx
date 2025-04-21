use {
    std::fs,
    crate::{
        cloud::{FxCloud, Service, ServiceId},
        storage::{SqliteStorage, NamespacedStorage, WithKey, BoxedStorage},
    },
};

mod cloud;
mod kafka;
mod storage;

#[tokio::main]
async fn main() {
    println!("starting fx...");

    let storage = SqliteStorage::in_memory();
    let fx_cloud = FxCloud::new()
        .with_code_storage(BoxedStorage::new(NamespacedStorage::new(b"services/", storage.clone()))
            .with_key(b"hello-service/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_hello_world.wasm").unwrap())
            .with_key(b"rpc-test-service/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_rpc_test_service.wasm").unwrap())
            .with_key(b"counter/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_counter.wasm").unwrap())
        )
        .with_storage(BoxedStorage::new(NamespacedStorage::new(b"data/", storage)))
        .with_service(Service::new(ServiceId::new("hello-service".to_owned())).with_env_var("demo/instance", "A"))
        .with_service(Service::new(ServiceId::new("rpc-test-service".to_owned())))
        .with_service(Service::new(ServiceId::new("counter".to_owned())).global());

    fx_cloud.run_http(8080, &ServiceId::new("hello-service".to_owned())).await;
}
