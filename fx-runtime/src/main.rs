// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::{path::PathBuf, sync::Arc, process::exit, net::SocketAddr, convert::Infallible, pin::Pin, cell::RefCell, rc::Rc},
    tracing::{Level, info, error, warn},
    tracing_subscriber::FmtSubscriber,
    clap::{Parser, Subcommand, ValueEnum, builder::PossibleValue},
    ::futures::{FutureExt, StreamExt, future::join_all, future::BoxFuture},
    hyper::{Response, body::Bytes, server::conn::http1, StatusCode},
    hyper_util::rt::{TokioIo, TokioTimer},
    http_body_util::{Full, BodyStream},
    walkdir::WalkDir,
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

            let config_path = std::env::current_dir().unwrap().join(config_file);
            info!("Loading config from {config_path:?}");
            let config = ServerConfig::load(config_path);

            let cpu_info = match gdt_cpus::cpu_info() {
                Ok(v) => Some(v),
                Err(err) => {
                    error!("failed to get cpu info: {err:?}");
                    None
                }
            };

            let management_thread_handle = std::thread::spawn(move || {
                info!("started management thread");

                let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let local_set = tokio::task::LocalSet::new();

                let definitions_monitor = DefinitionsMonitor::new(&config);

                tokio_runtime.block_on(local_set.run_until(async {
                    tokio::join!(
                        definitions_monitor.scan_definitions(),
                    )
                }));
            });

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

                    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let local_set = tokio::task::LocalSet::new();

                    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, Some(socket2::Protocol::TCP)).unwrap();
                    socket.set_reuse_port(true).unwrap();
                    socket.set_reuse_address(true).unwrap();
                    socket.set_nonblocking(true).unwrap();

                    // TODO: take port from config
                    let addr: SocketAddr = ([0, 0, 0, 0], 8080).into();
                    socket.bind(&addr.into()).unwrap();
                    socket.listen(1024).unwrap();

                    tokio_runtime.block_on(local_set.run_until(async {
                        let listener = tokio::net::TcpListener::from_std(socket.into()).unwrap();
                        let graceful = hyper_util::server::graceful::GracefulShutdown::new();

                        loop {
                            tokio::select! {
                                connection = listener.accept() => {
                                    let (tcp, _) = match connection {
                                        Ok(v) => v,
                                        Err(err) => {
                                            error!("failed to accept http connection: {err:?}");
                                            continue;
                                        }
                                    };
                                    info!(worker_id, "new http connection");

                                    let io = TokioIo::new(tcp);
                                    let conn = http1::Builder::new()
                                        .timer(TokioTimer::new())
                                        .serve_connection(io, HttpHandlerV2);
                                    let request_future = graceful.watch(conn);

                                    tokio::task::spawn_local(async move {
                                        if let Err(err) = request_future.await {
                                            if err.is_timeout() {
                                                // ignore timeouts, because those can be caused by client
                                            } else if err.is_incomplete_message() {
                                                // ignore incomplete messages, because those are caused by client
                                            } else {
                                                error!("error while handling http request: {err:?}"); // incomplete message should be fine
                                            }
                                        }
                                    });
                                }
                            }
                        }
                    }));
                });
                worker_handles.push(handle);
                worker_id += 1;
            }

            management_thread_handle.join().unwrap();
            for handle in worker_handles {
                handle.join().unwrap();
            }
        },
    }
}

struct HttpHandlerV2;

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for HttpHandlerV2 {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        Box::pin(HttpHandlerFuture::new(req))
    }
}

struct HttpHandlerFuture<'a> {
    inner: BoxFuture<'a, Result<Response<Full<Bytes>>, Infallible>>,
}

impl<'a> HttpHandlerFuture<'a> {
    fn new(req: hyper::Request<hyper::body::Incoming>) -> Self {
        let inner = Box::pin(async move {
            unimplemented!()
        });

        Self {
            inner,
        }
    }
}

impl<'a> Future for HttpHandlerFuture<'a> {
    type Output = Result<Response<Full<Bytes>>, Infallible>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        self.inner.poll_unpin(cx)
    }
}

struct DefinitionsMonitor {
    functions_directory: PathBuf,
}

impl DefinitionsMonitor {
    pub fn new(
        config: &ServerConfig,
    ) -> Self {
        Self {
            functions_directory: config.config_path.as_ref().unwrap().parent().unwrap().join(&config.functions_dir),
        }
    }

    async fn scan_definitions(&self) {
        info!("will scan definitions in: {:?}", self.functions_directory);

        let root = &self.functions_directory;
        for entry in WalkDir::new(root) {
            let entry = match entry {
                Ok(v) => v,
                Err(err) => {
                    warn!("failed to scan definitions in dir: {err:?}");
                    continue;
                }
            };

            if !entry.file_type().is_file() {
                continue;
            }

            unimplemented!()
        }
    }

}
