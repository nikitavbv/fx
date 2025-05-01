// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::fs,
    tracing::{Level, info, error},
    tracing_subscriber::FmtSubscriber,
    anyhow::anyhow,
    crate::{
        cloud::{FxCloud, Service, ServiceId},
        storage::{SqliteStorage, NamespacedStorage, WithKey, BoxedStorage},
        sql::SqlDatabase,
        config::Config,
    },
};

mod cloud;
mod compatibility;
mod compiler;
mod config;
mod cron;
mod error;
mod http;
mod kafka;
mod queue;
mod registry;
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
    let config = fs::read("./fx.yaml")
        .map_err(|err| anyhow!("failed to read config file: {err:?}"))?;
    let config = String::from_utf8(config)
        .map_err(|err| anyhow!("failed to parse config file: {err:?}"))?;
    let config = Config::load(&config);
    info!("loaded config: {config:?}");

    let code_storage = BoxedStorage::new(SqliteStorage::in_memory()
        .map_err(|err| anyhow!("failed to create storage: {err:?}"))?
        .with_key(b"dashboard", &fs::read("./target/wasm32-unknown-unknown/release/fx_cloud_dashboard.wasm")?)
        .with_key(b"dashboard-events-consumer", &fs::read("./target/wasm32-unknown-unknown/release/fx_cloud_dashboard.wasm")?)
        .with_key(b"hello-service", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_hello_world.wasm")?)
        .with_key(b"rpc-test-service", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_rpc_test_service.wasm")?)
        .with_key(b"counter", &fs::read("./target/wasm32-unknown-unknown/release/fx_app_counter.wasm")?));
    let compiler_storage = BoxedStorage::new(SqliteStorage::in_memory()?);

    let dashboard_database = SqlDatabase::in_memory();
    dashboard_database.exec(sql::Query::new("create table if not exists functions (function_id text primary key, total_invocations integer not null)".to_owned()));

    let fx_cloud = FxCloud::new()
        .with_code_storage(code_storage)
        .with_memoized_compiler(compiler_storage)
        .with_queue()
        .with_cron(SqlDatabase::in_memory())
        .with_service(Service::new(ServiceId::new("dashboard".to_owned())).with_sql_database("dashboard".to_owned(), dashboard_database.clone()))
        .with_service(Service::new(ServiceId::new("dashboard-events-consumer".to_owned())).system().with_sql_database("dashboard".to_owned(), dashboard_database))
        .with_service(
            Service::new(ServiceId::new("hello-service".to_owned()))
                .allow_fetch()
                .with_env_var("demo/instance", "A")
                .with_storage(BoxedStorage::new(NamespacedStorage::new("data/demo/".as_bytes().to_vec(), SqliteStorage::in_memory()?)))
                .with_sql_database("test-db".to_owned(), SqlDatabase::in_memory())
        )
        .with_service(Service::new(ServiceId::new("rpc-test-service".to_owned())))
        .with_service(Service::new(ServiceId::new("counter".to_owned())).global())
        .with_queue_subscription("system/invocations", ServiceId::new("dashboard-events-consumer".to_owned()), "on_invoke")
        .with_cron_task("*/10 * * * * * *", ServiceId::new("hello-service".to_owned()), "on_cron");

    fx_cloud.run_queue();
    fx_cloud.run_cron();
    fx_cloud.run_http(8080, &ServiceId::new("dashboard".to_owned())).await;

    Ok(())
}

async fn run_function(function_path: &str) -> anyhow::Result<()> {
    let storage = SqliteStorage::in_memory()
        .map_err(|err| anyhow!("failed to create storage: {err:?}"))?;
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
