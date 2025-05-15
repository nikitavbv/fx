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
        config::{Config, kv_from_config, sql_from_config},
        registry::{KVRegistry, SqlRegistry},
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
mod futures;
mod queue;
mod registry;
mod sql;
mod storage;
mod streams;

#[tokio::main]
async fn main() {
    FmtSubscriber::builder().with_max_level(Level::INFO).init();
    info!("starting fx...");
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        if let Err(err) = run_function(args[1].as_str()).await {
            error!("failed to run function: {err:?}");
        }
    } else if let Err(err) = run_demo().await {
        error!("failed to start fx cloud instance: {err:?}");
    }
}

async fn run_demo() -> anyhow::Result<()> {
    let config = fs::read("./fx.yaml")
        .map_err(|err| anyhow!("failed to read config file: {err:?}"))?;
    let config = String::from_utf8(config)
        .map_err(|err| anyhow!("failed to parse config file: {err:?}"))?;
    let config = Config::load(&config);
    info!("loaded config: {config:?}");

    let kv_registry = KVRegistry::new();
    for kv_config in config.kv {
        kv_registry.register(
            kv_config.id.clone(),
            kv_from_config(&kv_config).map_err(|err| anyhow!("failed to create kv storage: {err:?}"))?
        );
    }

    let sql_registry = SqlRegistry::new();
    for sql_config in config.sql {
        sql_registry.register(
            sql_config.id.clone(),
            sql_from_config(&sql_config)
                .map_err(|err| anyhow!("failed to create storage from config: {err:?}"))?,
        );
    }

    let fx_cloud = FxCloud::new()
        .with_code_storage(
            kv_registry.get("code".to_owned())
                .map_err(|err| anyhow!("failed to get code storage from registry: {err:?}"))?,
        )
        .with_memoized_compiler(
            kv_registry.get("compiler".to_owned())
                .map_err(|err| anyhow!("failed to get compiler storage from registry: {err:?}"))?,
        )
        .with_queue().await
        .with_cron(
            SqlDatabase::in_memory()
                .map_err(|err| anyhow!("failed to open database for cron: {err:?}"))?,
        ).map_err(|err| anyhow!("failed to setup cron: {err:?}"))?
        .with_service(Service::new(ServiceId::new("dashboard".to_owned())).with_sql_database("dashboard".to_owned(), sql_registry.get("dashboard".to_owned())))
        .with_service(Service::new(ServiceId::new("dashboard-events-consumer".to_owned())).system().with_sql_database("dashboard".to_owned(), sql_registry.get("dashboard".to_owned())))
        .with_service(
            Service::new(ServiceId::new("hello-service".to_owned()))
                .allow_fetch()
                .with_env_var("demo/instance", "A")
                .with_storage(BoxedStorage::new(NamespacedStorage::new("data/demo/".as_bytes().to_vec(), SqliteStorage::in_memory()?)))
                .with_sql_database(
                    "test-db".to_owned(),
                    SqlDatabase::in_memory()
                        .map_err(|err| anyhow!("failed to open database for demo service: {err:?}"))?,
                )
        )
        .with_service(Service::new(ServiceId::new("rpc-test-service".to_owned())))
        .with_service(Service::new(ServiceId::new("counter".to_owned())).global())
        .with_queue_subscription("system/invocations", ServiceId::new("dashboard-events-consumer".to_owned()), "on_invoke").await
        .with_cron_task("*/10 * * * * * *", ServiceId::new("hello-service".to_owned()), "on_cron");

    // fx_cloud.run_cron();
    fx_cloud.run_queue().await;
    fx_cloud.run_http(8080, ServiceId::new("dashboard".to_owned())).await;

    Ok(())
}

async fn run_function(function_path: &str) -> anyhow::Result<()> {
    let storage = SqliteStorage::in_memory()
        .map_err(|err| anyhow!("failed to create storage: {err:?}"))?;
    let fx_cloud = FxCloud::new()
        .with_code_storage(BoxedStorage::new(NamespacedStorage::new(b"services/", storage.clone()))
            .with_key(b"http/service.wasm", &fs::read(function_path)?)?
        )
        .with_service(
            Service::new(ServiceId::new("http".to_owned()))
                .with_storage(BoxedStorage::new(NamespacedStorage::new(b"data/", storage)))
        );

    fx_cloud.run_http(8080, ServiceId::new("http".to_owned())).await;

    Ok(())
}
