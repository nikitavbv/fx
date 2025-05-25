// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::{fs, path::PathBuf, net::SocketAddr, time::Duration, sync::Arc},
    tracing::{Level, info, error},
    tracing_subscriber::FmtSubscriber,
    anyhow::anyhow,
    tokio::{join, net::TcpListener, time::sleep},
    hyper::server::conn::http1,
    hyper_util::rt::{TokioIo, TokioTimer},
    clap::{Parser, Subcommand, CommandFactory},
    ::futures::StreamExt,
    crate::{
        cloud::{FxCloud, ServiceId, Engine},
        storage::{SqliteStorage, NamespacedStorage, WithKey, BoxedStorage, FsStorage, SuffixStorage, KVStorage},
        sql::SqlDatabase,
        definition::{DefinitionProvider, load_cron_task_from_config},
        registry::{KVRegistry, SqlRegistry},
        http::HttpHandler,
        cron::{CronRunner, CronTaskDefinition},
    },
};

mod cloud;
mod compatibility;
mod compiler;
mod cron;
mod definition;
mod error;
mod http;
mod kafka;
mod futures;
mod metrics;
mod registry;
mod sql;
mod storage;
mod streams;

const FILE_EXTENSION_WASM: &str = ".wasm";
const FILE_EXTENSION_DEFINITION: &str = ".fx.yaml";

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
        function: String,

        #[arg(long)]
        port: Option<u16>,
    },
    Cron {
        schedule_file: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    FmtSubscriber::builder().with_max_level(Level::INFO).init();
    let args = Args::parse();

    let functions_dir = args.functions_dir.map(|v| PathBuf::from(v)).unwrap_or(std::env::current_dir().unwrap());
    let code_storage = BoxedStorage::new(SuffixStorage::new(FILE_EXTENSION_WASM, FsStorage::new(functions_dir.clone())));
    let definition_storage = BoxedStorage::new(SuffixStorage::new(FILE_EXTENSION_DEFINITION, FsStorage::new(functions_dir)));
    let definition_provider = DefinitionProvider::new(definition_storage.clone());

    let fx_cloud = FxCloud::new()
        .with_code_storage(code_storage.clone())
        .with_definition_provider(definition_provider);

    tokio::join!(
        reload_on_key_changes(fx_cloud.engine.clone(), code_storage),
        reload_on_key_changes(fx_cloud.engine.clone(), definition_storage),
        run_command(fx_cloud, args.command),
    );
}

async fn run_command(fx_cloud: FxCloud, command: Command) {
    match command {
        Command::Run { function, rpc_method_name } => {
            let function = if function.ends_with(FILE_EXTENSION_WASM) {
                &function[0..function.len() - FILE_EXTENSION_WASM.len()]
            } else {
                &function
            };
            fx_cloud.invoke_service::<(), ()>(&ServiceId::new(function), &rpc_method_name, ()).await.unwrap();
        },
        Command::Http { function, port } => {
            let addr: SocketAddr = ([0, 0, 0, 0], port.unwrap_or(8080)).into();
            let listener = TcpListener::bind(addr).await.unwrap();
            let http_handler = HttpHandler::new(fx_cloud, ServiceId::new(function));
            info!("running http server on {addr:?}");
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let io = TokioIo::new(tcp);

                let http_handler = http_handler.clone();
                tokio::task::spawn(async move {
                    if let Err(err) = http1::Builder::new()
                        .timer(TokioTimer::new())
                        .serve_connection(io, http_handler)
                        .await {
                            if err.is_timeout() {
                                // ignore timeouts, because those can be caused by client
                            } else {
                                error!("error while handling http request: {err:?}");
                            }
                        }
                });
            }
        },
        Command::Cron { schedule_file } => {
            let config = load_cron_task_from_config(fs::read(schedule_file).unwrap());
            let database = SqlDatabase::new(config.state_path).unwrap();
            let cron_runner = {
                let mut cron_runner = CronRunner::new(fx_cloud.engine.clone(), database);

                for task in config.tasks {
                    cron_runner = cron_runner.with_task(
                        CronTaskDefinition::new(task.id)
                            .with_schedule(task.schedule)
                            .with_function_id(task.function)
                            .with_method_name(task.rpc_method_name)
                    );
                }

                cron_runner
            };

            cron_runner.run().await;
        }
    }
}

async fn reload_on_key_changes(engine: Arc<Engine>, storage: BoxedStorage) {
    let mut watcher = storage.watch();
    while let Some(key) = watcher.next().await {
        engine.reload(&ServiceId::new(String::from_utf8(key.key).unwrap()))
    }
}
