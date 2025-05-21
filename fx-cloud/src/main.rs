// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::{fs, path::PathBuf},
    tracing::{Level, info, error},
    tracing_subscriber::FmtSubscriber,
    anyhow::anyhow,
    tokio::join,
    clap::{Parser, Subcommand, CommandFactory},
    crate::{
        cloud::{FxCloud, Service, ServiceId},
        storage::{SqliteStorage, NamespacedStorage, WithKey, BoxedStorage, FsStorage, SuffixStorage},
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
mod metrics;
mod queue;
mod registry;
mod sql;
mod storage;
mod streams;

const FILE_EXTENSION_WASM: &str = ".wasm";

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,

    #[arg(long)]
    functions_dir: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Run {
        function: String,
        rpc_method_name: String,
    },
    Http {
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    FmtSubscriber::builder().with_max_level(Level::INFO).init();
    let args = Args::parse();

    let functions_dir = args.functions_dir.map(|v| PathBuf::from(v)).unwrap_or(std::env::current_dir().unwrap());
    let code_storage = SuffixStorage::new(FILE_EXTENSION_WASM, FsStorage::new(functions_dir));

    let fx_cloud = FxCloud::new()
        .with_code_storage(BoxedStorage::new(code_storage));

    match args.command {
        Command::Run { function, rpc_method_name } => {
            let function = if function.ends_with(FILE_EXTENSION_WASM) {
                &function[0..function.len() - FILE_EXTENSION_WASM.len()]
            } else {
                &function
            };
            fx_cloud
                .with_service(Service::new(ServiceId::new(function)))
                .invoke_service::<(), ()>(&ServiceId::new(function), &rpc_method_name, ()).await.unwrap();
        },
        Command::Http {} => {
            println!("http server not implemented yet");
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

    let kv_registry = KVRegistry::new();
    for kv_config in config.kv {
        kv_registry.register(
            kv_config.id.clone(),
            kv_from_config(&kv_config).map_err(|err| anyhow!("failed to create kv storage: {err:?}"))?
        ).map_err(|err| anyhow!("failed to register kv: {err:?}"))?;
    }

    let sql_registry = SqlRegistry::new();
    for sql_config in config.sql {
        sql_registry.register(
            sql_config.id.clone(),
            sql_from_config(&sql_config)
                .map_err(|err| anyhow!("failed to create storage from config: {err:?}"))?,
        );
    }

    let demo_storage = BoxedStorage::new(SqliteStorage::in_memory()?)
        .with_key("demo/instance".as_bytes(), "A".as_bytes())?;

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
                .with_storage("demo".to_owned(), demo_storage)
                .with_sql_database(
                    "test-db".to_owned(),
                    SqlDatabase::in_memory()
                        .map_err(|err| anyhow!("failed to open database for demo service: {err:?}"))?,
                )
        )
        .with_service(Service::new(ServiceId::new("rpc-test-service".to_owned())))
        .with_service(Service::new(ServiceId::new("counter".to_owned())))
        .with_queue_subscription("system/invocations", ServiceId::new("dashboard-events-consumer".to_owned()), "on_invoke").await
        .with_cron_task("*/10 * * * * * *", ServiceId::new("hello-service".to_owned()), "on_cron")?;

    // fx_cloud.run_cron();
    join!(
        fx_cloud.run_queue(),
        fx_cloud.run_http(8080, ServiceId::new("dashboard".to_owned())),
        fx_cloud.run_metrics_server(8081),
    );

    Ok(())
}
