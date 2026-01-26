// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::{fs, path::PathBuf, sync::Arc, process::exit},
    tracing::{Level, info, error, warn},
    tracing_subscriber::FmtSubscriber,
    clap::{Parser, Subcommand, ValueEnum, builder::PossibleValue},
    ::futures::{FutureExt, StreamExt, future::join_all},
    crate::{
        runtime::{
            runtime::{FxRuntime, FunctionId, Engine},
            kv::{BoxedStorage, FsStorage, SuffixStorage, KVStorage},
            definition::{DefinitionProvider, load_rabbitmq_consumer_task_from_config},
            metrics::run_metrics_server,
            logs::{BoxLogger, StdoutLogger, NoopLogger},
        },
        server::{
            consumer::RabbitMqConsumer,
            logs::RabbitMqLogger,
            server::FxServer,
            config::ServerConfig,
        },
    },
};

mod common;
mod runtime;
mod server;

const FILE_EXTENSION_WASM: &str = ".wasm";

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,

    #[arg(long)]
    metrics_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub enum ArgsLogger {
    Stdout,
    Noop,
    RabbitMq,
}

impl ValueEnum for ArgsLogger {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Stdout, Self::Noop, Self::RabbitMq]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            Self::Stdout => PossibleValue::new("stdout").help("write logs to stdout"),
            Self::Noop => PossibleValue::new("noop").help("do not write logs anywhere"),
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
    Serve {
        config_file: String,
    },
}

impl Command {
    pub fn is_serve(&self) -> bool {
        match &self {
            Self::Serve { .. } => true,
            _ => false,
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    FmtSubscriber::builder().with_max_level(Level::INFO).init();
    let args = Args::parse();

    let fx_runtime = FxRuntime::new();

    // TODO: move to server, based on ServerConfig
    tokio::spawn(run_metrics_server(fx_runtime.engine.clone(), args.metrics_port.unwrap_or(8081)));

    run_command(fx_runtime, args.command).await;
}

async fn run_command(fx_runtime: FxRuntime, command: Command) {
    match command {
        Command::Run { function, rpc_method_name } => {
            // TODO: make run work with FxServer
            let function = if function.ends_with(FILE_EXTENSION_WASM) {
                &function[0..function.len() - FILE_EXTENSION_WASM.len()]
            } else {
                &function
            };
            let result = fx_runtime.invoke_service::<(), ()>(&FunctionId::new(function), &rpc_method_name, ()).await;
            if let Err(err) = result {
                error!("failed to invoke function: {err:?}");
                exit(-1);
            }
        },
        Command::Serve { config_file } => {
            let config_path = std::env::current_dir().unwrap().join(config_file);
            info!("Loading config from {config_path:?}");
            let config = ServerConfig::load(config_path).await;

            FxServer::new(
                config,
                fx_runtime,
            )
                .await
                .serve()
                .await
        },
    }
}
