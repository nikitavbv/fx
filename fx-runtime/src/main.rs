// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::{path::{PathBuf, Path}, collections::HashMap, net::SocketAddr, convert::Infallible, pin::Pin, rc::Rc},
    tracing::{Level, info, error, warn},
    tracing_subscriber::FmtSubscriber,
    tokio::{fs, sync::oneshot},
    clap::{Parser, Subcommand, ValueEnum, builder::PossibleValue},
    ::futures::{FutureExt, StreamExt, future::join_all, future::BoxFuture},
    hyper::{Response, body::Bytes, server::conn::http1, StatusCode},
    hyper_util::rt::{TokioIo, TokioTimer},
    http_body_util::{Full, BodyStream},
    walkdir::WalkDir,
    thiserror::Error,
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
            config::{ServerConfig, FunctionConfig, FunctionCodeConfig},
        },
    },
};

mod common;
mod introspection;
mod runtime;
mod server;

const FILE_EXTENSION_WASM: &str = ".wasm";
const DEFINITION_FILE_SUFFIX: &str = ".fx.yaml";

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

#[derive(Debug)]
enum WorkerMessage {
    RemoveFunction(FunctionId),
    FunctionDeploy {
        function_id: FunctionId,
        instance_id: FunctionInstanceId,
        module: wasmtime::Module,

        http_listeners: Vec<FunctionHttpListener>,
    }
}

struct CompilerMessage {
    function_id: FunctionId,
    code: Vec<u8>,
    response: oneshot::Sender<wasmtime::Module>,
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
            let wasmtime = wasmtime::Engine::new(
                wasmtime::Config::new()
                    .async_support(true)
            ).unwrap();

            let cpu_info = match gdt_cpus::cpu_info() {
                Ok(v) => Some(v),
                Err(err) => {
                    error!("failed to get cpu info: {err:?}");
                    None
                }
            };

            let worker_threads = 4.min(cpu_info.map(|v| v.num_logical_cores()).unwrap_or(usize::MAX));
            let (workers_tx, workers_rx) = (0..worker_threads)
                .map(|_| flume::unbounded::<WorkerMessage>())
                .unzip::<_, _, Vec<_>, Vec<_>>();
            let (compiler_tx, compiler_rx) = flume::unbounded::<CompilerMessage>();

            let management_thread_handle = std::thread::spawn(move || {
                info!("started management thread");

                let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let local_set = tokio::task::LocalSet::new();

                let mut definitions_monitor = DefinitionsMonitor::new(&config, workers_tx, compiler_tx);

                tokio_runtime.block_on(local_set.run_until(async {
                    tokio::join!(
                        definitions_monitor.scan_definitions(),
                    )
                }));
            });

            let compiler_thread_handle = std::thread::spawn(move || {
                info!("started compiler thread");

                while let Ok(msg) = compiler_rx.recv() {
                    let function_id = msg.function_id.as_string();

                    info!(function_id, "compiling");
                    msg.response.send(wasmtime::Module::new(&wasmtime, msg.code).unwrap()).unwrap();
                    info!(function_id, "done compiling");
                }
            });

            let worker_cores = cpu_info.as_ref()
                .map(|v| v.logical_processor_ids().iter().take(worker_threads).map(|v| Some(*v)).collect::<Vec<_>>())
                .unwrap_or(std::iter::repeat(None).take(worker_threads).collect());

            struct WorkerConfig {
                core_id: Option<usize>,
                messages_rx: flume::Receiver<WorkerMessage>,
            }

            let workers = worker_cores.into_iter()
                .zip(workers_rx.into_iter())
                .map(|(core_id, messages_rx)| WorkerConfig {
                    core_id,
                    messages_rx,
                })
                .collect::<Vec<_>>();

            let mut worker_id = 0;
            let mut worker_handles = Vec::new();
            for worker in workers.into_iter() {
                let handle = std::thread::spawn(move || {
                    let mut worker = worker;

                    if let Some(worker_core_id) = worker.core_id {
                        match gdt_cpus::pin_thread_to_core(worker_core_id) {
                            Ok(_) => {},
                            Err(gdt_cpus::Error::Unsupported(_)) => {},
                            Err(err) => error!("failed to pin thread to core: {err:?}"),
                        }
                    }

                    info!(worker_id, "started worker thread");

                    // setup async runtime:
                    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let local_set = tokio::task::LocalSet::new();

                    // setup socket:
                    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, Some(socket2::Protocol::TCP)).unwrap();
                    socket.set_reuse_port(true).unwrap();
                    socket.set_reuse_address(true).unwrap();
                    socket.set_nonblocking(true).unwrap();

                    // TODO: take port from config
                    let addr: SocketAddr = ([0, 0, 0, 0], 8080).into();
                    socket.bind(&addr.into()).unwrap();
                    socket.listen(1024).unwrap();

                    // setup wasm runtime:
                    let mut function_instances: HashMap<FunctionInstanceId, FunctionInstance> = HashMap::new();
                    let mut functions: HashMap<FunctionId, FunctionInstanceId> = HashMap::new();
                    let mut http_hosts: HashMap<String, FunctionId> = HashMap::new();
                    let mut http_default: Option<FunctionId> = None;

                    // run worker:
                    tokio_runtime.block_on(local_set.run_until(async {
                        let listener = tokio::net::TcpListener::from_std(socket.into()).unwrap();
                        let graceful = hyper_util::server::graceful::GracefulShutdown::new();

                        loop {
                            tokio::select! {
                                message = worker.messages_rx.recv_async() => {
                                    match message.unwrap() {
                                        WorkerMessage::FunctionDeploy {
                                            function_id,
                                            instance_id,
                                            module,
                                            http_listeners,
                                        } => {
                                            function_instances.insert(instance_id.clone(), FunctionInstance::new(module));
                                            // TODO: cleanup old instances
                                            functions.insert(function_id.clone(), instance_id);

                                            http_hosts = http_hosts.into_iter()
                                                .filter(|v| &v.1 != &function_id)
                                                .chain(http_listeners.iter().filter_map(|v| v.host.as_ref()).map(|v| (v.clone(), function_id.clone())))
                                                .collect();

                                            if http_listeners.iter().find(|v| v.host.is_none()).is_some() {
                                                http_default = Some(function_id.clone());
                                            }
                                        },
                                        other => unimplemented!("unsupported message: {other:?}"),
                                    }
                                },

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

            for handle in worker_handles {
                handle.join().unwrap();
            }
            compiler_thread_handle.join().unwrap();
            management_thread_handle.join().unwrap();
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
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
    compiler_tx: flume::Sender<CompilerMessage>,

    // DefinitionsMonitor requests FunctionInstance creation and assigns IDs to them.
    function_instance_id_counter: u64,
}

impl DefinitionsMonitor {
    pub fn new(
        config: &ServerConfig,
        workers_tx: Vec<flume::Sender<WorkerMessage>>,
        compiler_tx: flume::Sender<CompilerMessage>,
    ) -> Self {
        Self {
            functions_directory: config.config_path.as_ref().unwrap().parent().unwrap()
                .join(&config.functions_dir),
            workers_tx,
            compiler_tx,
            function_instance_id_counter: 0,
        }
    }

    async fn scan_definitions(&mut self) {
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

            let entry_path = entry.path();
            let function_id = match self.path_to_function_id(entry_path) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let function_config = match FunctionConfig::load(entry_path.to_path_buf()).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to load function config: {err:?}");
                    continue;
                }
            };

            self.apply_config(function_id, function_config).await;
        }

        info!("listening for definition changes");
        let (tx, mut rx) = flume::unbounded();
        let event_fn = {
            move |res: notify::Result<notify::Event>| {
                let res = res.unwrap();

                match res.kind {
                    notify::EventKind::Access(_) => {},
                    _other => {
                        for changed_path in res.paths {
                            tx.send(changed_path).unwrap();
                        }
                    }
                }
            }
        };
        let mut watcher = notify::recommended_watcher(event_fn).unwrap();

        while let Ok(path) = rx.recv_async().await {
            let function_id = match self.path_to_function_id(&path) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !fs::try_exists(&path).await.unwrap() {
                self.remove_function(function_id);
                continue;
            }

            let metadata = fs::metadata(&path).await.unwrap();
            if !metadata.is_file() {
                continue;
            }

            let function_config = match FunctionConfig::load(path).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to load function config: {err:?}");
                    continue
                }
            };

            self.apply_config(function_id, function_config).await;
        }
    }

    fn path_to_function_id(&self, path: &Path) -> Result<FunctionId, FunctionIdDetectionError> {
        let function_id = path.strip_prefix(&self.functions_directory).unwrap().to_str().unwrap();
        if !function_id.ends_with(DEFINITION_FILE_SUFFIX) {
            return Err(FunctionIdDetectionError::PathMissingExtension);
        }
        let function_id = &function_id[0..function_id.len() - DEFINITION_FILE_SUFFIX.len()];
        Ok(FunctionId::new(function_id))
    }

    fn function_id_to_path(&self, function_id: &FunctionId) -> PathBuf {
        self.functions_directory.join(function_id.as_string())
    }

    fn remove_function(&self, function_id: FunctionId) {
        info!("removing function: {:?}", function_id.as_string());
        for worker in self.workers_tx.iter() {
            worker.send(WorkerMessage::RemoveFunction(function_id.clone())).unwrap();
        }
    }

    async fn apply_config(&mut self, function_id: FunctionId, config: FunctionConfig) {
        info!("applying config for: {:?}", function_id.as_string());

        // first, precompile module:
        let module_code = match config.code {
            Some(v) => match v {
                FunctionCodeConfig::Path(v) => fs::read(&v).await.unwrap(),
                FunctionCodeConfig::Inline(v) => v,
            },
            None => {
                let module_code = self.function_id_to_path(&function_id).with_added_extension("wasm");
                fs::read(&module_code).await.unwrap()
            }
        };

        let (compiler_response_tx, compiler_response_rx) = oneshot::channel();
        self.compiler_tx.send_async(CompilerMessage {
            function_id: function_id.clone(),
            code: module_code,
            response: compiler_response_tx,
        }).await.unwrap();

        let instance_id = self.next_instance_id();
        let module = compiler_response_rx.await.unwrap();

        let http_listeners = config.triggers.iter()
            .flat_map(|v| v.http.iter())
            .flat_map(|v| v.iter())
            .map(|v| FunctionHttpListener {
                host: v.host.clone(),
            })
            .collect::<Vec<_>>();

        for worker in self.workers_tx.iter() {
            worker.send_async(WorkerMessage::FunctionDeploy {
                function_id: function_id.clone(),
                instance_id: instance_id.clone(),
                module: module.clone(),
                http_listeners: http_listeners.clone(),
            }).await.unwrap();
        }
    }

    fn next_instance_id(&mut self) -> FunctionInstanceId {
        let instance_id = FunctionInstanceId::new(self.function_instance_id_counter);
        self.function_instance_id_counter += 1;
        instance_id
    }
}

#[derive(Error, Debug)]
enum FunctionIdDetectionError {
    #[error("config path missing .fx.yaml extension")]
    PathMissingExtension,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct FunctionInstanceId {
    id: u64,
}

impl FunctionInstanceId {
    fn new(id: u64) -> Self {
        Self { id }
    }
}

struct FunctionInstance {
    module: wasmtime::Module,
}

impl FunctionInstance {
    pub fn new(module: wasmtime::Module) -> Self {
        Self {
            module,
        }
    }
}

#[derive(Clone, Debug)]
struct FunctionHttpListener {
    host: Option<String>,
}

impl FunctionHttpListener {
    pub fn new(host: Option<String>) -> Self {
        Self {
            host,
        }
    }
}
