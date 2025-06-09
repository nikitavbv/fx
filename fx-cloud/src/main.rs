// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::{fs, path::PathBuf, net::SocketAddr, sync::Arc, process::exit},
    tracing::{Level, info, error, warn},
    tracing_subscriber::FmtSubscriber,
    anyhow::anyhow,
    tokio::{join, net::TcpListener, time::sleep},
    hyper::server::conn::http1,
    hyper_util::rt::{TokioIo, TokioTimer},
    clap::{Parser, Subcommand, CommandFactory, ValueEnum, builder::PossibleValue},
    ::futures::{FutureExt, StreamExt, future::join_all},
    crate::{
        cloud::{FxCloud, ServiceId, Engine},
        kv::{SqliteStorage, BoxedStorage, FsStorage, SuffixStorage, KVStorage},
        sql::SqlDatabase,
        definition::{DefinitionProvider, load_cron_task_from_config, load_rabbitmq_consumer_task_from_config},
        http::HttpHandler,
        cron::{CronRunner, CronTaskDefinition},
        metrics::run_metrics_server,
        logs::{BoxLogger, StdoutLogger, RabbitMqLogger},
        consumer::RabbitMqConsumer,
    },
};

mod cloud;
mod compatibility;
mod compiler;
mod consumer;
mod cron;
mod definition;
mod error;
mod http;
mod kafka;
mod futures;
mod metrics;
mod sql;
mod kv;
mod logs;
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

    #[arg(long)]
    metrics_port: Option<u16>,

    #[arg(long)]
    logger: Option<ArgsLogger>,

    #[arg(long)]
    logger_rabbitmq_uri: Option<String>,

    #[arg(long)]
    logger_rabbitmq_exchange: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ArgsLogger {
    Stdout,
    RabbitMq,
}

impl ValueEnum for ArgsLogger {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Stdout, Self::RabbitMq]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            Self::Stdout => PossibleValue::new("stdout").help("write logs to stdout"),
            Self::RabbitMq => PossibleValue::new("rabbitmq").help("write logs to rabbitmq exchange"),
        })
    }
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
    #[command(name = "rabbitmq")]
    RabbitMq {
        consumer_file: String,

        #[arg(long)]
        amqp_addr: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    FmtSubscriber::builder().with_max_level(Level::INFO).init();
    let args = Args::parse();
    let current_dir = match std::env::current_dir() {
        Ok(v) => v,
        Err(err) => {
            error!("failed to detect workdir: {err:?}");
            exit(-1);
        }
    };

    let functions_dir = args.functions_dir.map(PathBuf::from).unwrap_or(current_dir);
    let code_storage = BoxedStorage::new(SuffixStorage::new(FILE_EXTENSION_WASM, FsStorage::new(functions_dir.clone())));
    let definition_storage = BoxedStorage::new(SuffixStorage::new(FILE_EXTENSION_DEFINITION, FsStorage::new(functions_dir)));
    let definition_provider = DefinitionProvider::new(definition_storage.clone());

    let fx_cloud = FxCloud::new()
        .with_code_storage(code_storage.clone())
        .with_definition_provider(definition_provider);

    let fx_cloud = if let Some(logger) = args.logger {
        fx_cloud.with_logger(match logger {
            ArgsLogger::Stdout => BoxLogger::new(StdoutLogger::new()),
            ArgsLogger::RabbitMq => {
                let uri = match args.logger_rabbitmq_uri {
                    Some(v) => v,
                    None => {
                        error!("rabbitmq logger requires setting rabbitmq uri");
                        exit(-1);
                    }
                };

                let exchange = match args.logger_rabbitmq_exchange {
                    Some(v) => v,
                    None => {
                        error!("rabbitmq logger requires setting exchange param");
                        exit(-1);
                    }
                };

                BoxLogger::new(RabbitMqLogger::new(uri, exchange))
            },
        })
    } else {
        fx_cloud
    };

    tokio::spawn(run_metrics_server(fx_cloud.engine.clone(), args.metrics_port.unwrap_or(8081)));
    tokio::spawn(reload_on_key_changes(fx_cloud.engine.clone(), code_storage));
    tokio::spawn(reload_on_key_changes(fx_cloud.engine.clone(), definition_storage));

    run_command(fx_cloud, args.command).await;
}

async fn run_command(fx_cloud: FxCloud, command: Command) {
    match command {
        Command::Run { function, rpc_method_name } => {
            let function = if function.ends_with(FILE_EXTENSION_WASM) {
                &function[0..function.len() - FILE_EXTENSION_WASM.len()]
            } else {
                &function
            };
            let result = fx_cloud.invoke_service::<(), ()>(&ServiceId::new(function), &rpc_method_name, ()).await;
            if let Err(err) = result {
                error!("failed to invoke function: {err:?}");
                exit(-1);
            }
        },
        Command::Http { function, port } => {
            let addr: SocketAddr = ([0, 0, 0, 0], port.unwrap_or(8080)).into();
            let listener = match TcpListener::bind(addr).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to bind tcp listener for http sever: {err:?}");
                    exit(-1);
                }
            };
            let http_handler = HttpHandler::new(fx_cloud, ServiceId::new(function));
            info!("running http server on {addr:?}");
            loop {
                let (tcp, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(err) => {
                        error!("failed to accept http connection: {err:?}");
                        continue;
                    }
                };
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
            let config = match fs::read(schedule_file) {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to read cron config file: {err:?}");
                    exit(-1);
                }
            };
            let config = load_cron_task_from_config(config);
            let database = match SqlDatabase::new(config.state_path) {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to open database for cron state: {err:?}");
                    exit(-1);
                }
            };
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
        },
        Command::RabbitMq { consumer_file, amqp_addr } => {
            let config = match fs::read(consumer_file) {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to read consumer config file: {err:?}");
                    exit(-1);
                }
            };
            let config = load_rabbitmq_consumer_task_from_config(config);
            let consumers = config.consumers.into_iter()
                .map(|v| async {
                    RabbitMqConsumer::new(fx_cloud.engine.clone(), amqp_addr.clone(), v.id, v.queue, v.function, v.rpc_method_name).run().await
                }.boxed())
                .collect::<Vec<_>>();

            join_all(consumers).await;
        },
    }
}

async fn reload_on_key_changes(engine: Arc<Engine>, storage: BoxedStorage) {
    let mut watcher = storage.watch();
    while let Some(key) = watcher.next().await {
        let function_id = match String::from_utf8(key.key) {
            Ok(v) => v,
            Err(err) => {
                warn!("failed to decode function id as string: {err:?}");
                continue;
            }
        };
        engine.reload(&ServiceId::new(function_id))
    }
}
