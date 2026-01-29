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
            server::FxServer,
            config::ServerConfig,
        },
    },
};

mod common;
mod introspection;
mod runtime;
mod server;

const FILE_EXTENSION_WASM: &str = ".wasm";

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Serve {
        config_file: String,
    },
}

#[cfg(not(target_arch = "aarch64"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    FmtSubscriber::builder().with_max_level(Level::INFO).init();
    let args = Args::parse();

    match args.command {
        Command::Serve { config_file } => {
            warn!("note: fx is currently undergoing major refactoring. For now, please use versions 0.1.560 or older.");

            let cpu_info = match gdt_cpus::cpu_info() {
                Ok(v) => Some(v),
                Err(err) => {
                    error!("failed to get cpu info: {err:?}");
                    None
                }
            };

            let worker_threads = 4.min(cpu_info.map(|v| v.num_logical_cores()).unwrap_or(usize::MAX));
            let worker_cores = cpu_info.as_ref()
                .map(|v| v.logical_processor_ids().iter().take(worker_threads).map(|v| Some(*v)).collect::<Vec<_>>())
                .unwrap_or(std::iter::repeat(None).take(worker_threads).collect());

            let mut worker_id = 0;
            let mut worker_handles = Vec::new();
            for worker_core_id in worker_cores.into_iter() {
                let handle = std::thread::spawn(move || {
                    if let Some(worker_core_id) = worker_core_id {
                        match gdt_cpus::pin_thread_to_core(worker_core_id) {
                            Ok(_) => {},
                            Err(gdt_cpus::Error::Unsupported(_)) => {},
                            Err(err) => error!("failed to pin thread to core: {err:?}"),
                        }
                    }

                    info!(worker_id, "started worker thread");
                });
                worker_handles.push(handle);
                worker_id += 1;
            }

            for handle in worker_handles {
                handle.join().unwrap();
            }

            /*let config_path = std::env::current_dir().unwrap().join(config_file);
            info!("Loading config from {config_path:?}");
            let config = ServerConfig::load(config_path).await;

            FxServer::new(
                config,
                fx_runtime,
            )
                .await
                .serve()
                .await*/
        },
    }
}
