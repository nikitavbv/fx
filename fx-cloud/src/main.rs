// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::fs,
    tracing::{Level, info, error},
    tracing_subscriber::FmtSubscriber,
    crate::{
        cloud::{FxCloud, Service, ServiceId},
        storage::{SqliteStorage, NamespacedStorage, WithKey, BoxedStorage},
    },
};

mod cloud;
mod compatibility;
mod error;
mod http;
mod kafka;
mod sql;
mod storage;

#[tokio::main]
async fn main() {
    FmtSubscriber::builder().with_max_level(Level::INFO).init();
    info!("starting fx...");
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        if let Err(err) = run_function(args[1].as_str()).await {
            error!("failed to run function: {err:?}");
        }
    } else {
        if let Err(err) = run_demo().await {
            error!("failed to start fx cloud instance: {err:?}");
        }
    }
}

async fn run_demo() -> anyhow::Result<()> {
    let storage = SqliteStorage::in_memory()
        .with_key(b"services/dashboard/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_cloud_dashboard.wasm")?)
        .with_key(b"services/hello-service/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_hello_world.wasm")?)
        .with_key(b"services/rpc-test-service/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_rpc_test_service.wasm")?)
        .with_key(b"services/counter/service.wasm", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_counter.wasm")?);
    let fx_cloud = FxCloud::new()
        .with_code_storage(BoxedStorage::new(NamespacedStorage::new(b"services/", storage.clone())))
        .with_service(Service::new(ServiceId::new("dashboard".to_owned())))
        .with_service(
            Service::new(ServiceId::new("hello-service".to_owned()))
                .allow_fetch()
                .with_env_var("demo/instance", "A")
                .with_storage(BoxedStorage::new(NamespacedStorage::new("data/demo/".as_bytes().to_vec(), storage.clone())))
        )
        .with_service(Service::new(ServiceId::new("rpc-test-service".to_owned())))
        .with_service(Service::new(ServiceId::new("counter".to_owned())).global());

    fx_cloud.run_http(8080, &ServiceId::new("dashboard".to_owned())).await;

    Ok(())
}

async fn run_function(function_path: &str) -> anyhow::Result<()> {
    let storage = SqliteStorage::in_memory();
    let fx_cloud = FxCloud::new()
        .with_code_storage(BoxedStorage::new(NamespacedStorage::new(b"services/", storage.clone()))
            .with_key(b"http/service.wasm", &fs::read(function_path)?)
        )
        .with_service(
            Service::new(ServiceId::new("http".to_owned()))
                .with_storage(BoxedStorage::new(NamespacedStorage::new(b"data/", storage)))
        );

    fx_cloud.run_http(8080, &ServiceId::new("http".to_owned())).await;

    Ok(())
}
