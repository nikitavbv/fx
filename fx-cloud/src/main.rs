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
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        run_function(args[1].as_str()).await;
    } else {
        run_demo().await;
    }
}

async fn run_demo() {
    let storage = SqliteStorage::in_memory();
    let fx_cloud = FxCloud::new()
        .with_code_storage(BoxedStorage::new(NamespacedStorage::new(b"services/", storage.clone()))
            .with_key(b"hello-service/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_hello_world.wasm").unwrap())
            .with_key(b"rpc-test-service/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_rpc_test_service.wasm").unwrap())
            .with_key(b"counter/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_counter.wasm").unwrap())
        )
        .with_service(
            Service::new(ServiceId::new("hello-service".to_owned()))
                .with_env_var("demo/instance", "A")
                .with_storage(BoxedStorage::new(NamespacedStorage::new("data/demo/".as_bytes().to_vec(), storage.clone())))
        )
        .with_service(Service::new(ServiceId::new("rpc-test-service".to_owned())))
        .with_service(Service::new(ServiceId::new("counter".to_owned())).global());

    fx_cloud.run_http(8080, &ServiceId::new("hello-service".to_owned())).await;
}

async fn run_function(function_path: &str) {
    let storage = SqliteStorage::in_memory();
    let fx_cloud = FxCloud::new()
        .with_code_storage(BoxedStorage::new(NamespacedStorage::new(b"services/", storage.clone()))
            .with_key(b"http/service.wasm", &fs::read(function_path).unwrap())
        )
        .with_service(
            Service::new(ServiceId::new("http".to_owned()))
                .with_storage(BoxedStorage::new(NamespacedStorage::new(b"data/", storage)))
        );

    fx_cloud.run_http(8080, &ServiceId::new("http".to_owned())).await;
}
