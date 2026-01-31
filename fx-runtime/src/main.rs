// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

use {
    std::{
        path::{PathBuf, Path},
        collections::HashMap,
        net::SocketAddr,
        convert::Infallible,
        pin::Pin,
        rc::Rc,
        cell::RefCell,
    },
    tracing::{Level, info, error, warn},
    tracing_subscriber::FmtSubscriber,
    tokio::{fs, sync::oneshot},
    clap::{Parser, Subcommand, ValueEnum, builder::PossibleValue},
    ::futures::{FutureExt, StreamExt, future::join_all, future::LocalBoxFuture},
    hyper::{Response, body::Bytes, server::conn::http1, StatusCode},
    hyper_util::rt::{TokioIo, TokioTimer},
    http_body_util::{Full, BodyStream},
    walkdir::WalkDir,
    thiserror::Error,
    notify::Watcher,
    fx_types::{capnp, abi_capnp},
    wasmtime::AsContextMut,
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
        deployment_id: FunctionDeploymentId,
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

            let compiler_thread_handle = {
                let wasmtime = wasmtime.clone();

                std::thread::spawn(move || {
                    info!("started compiler thread");

                    while let Ok(msg) = compiler_rx.recv() {
                        let function_id = msg.function_id.as_string();

                        info!(function_id, "compiling");
                        msg.response.send(wasmtime::Module::new(&wasmtime, msg.code).unwrap()).unwrap();
                        info!(function_id, "done compiling");
                    }
                })
            };

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
                let wasmtime = wasmtime.clone();

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
                    let function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>> = Rc::new(RefCell::new(HashMap::new()));
                    let functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>> = Rc::new(RefCell::new(HashMap::new()));
                    let http_hosts: Rc<RefCell<HashMap<String, FunctionId>>> = Rc::new(RefCell::new(HashMap::new()));
                    let http_default: Rc<RefCell<Option<FunctionId>>> = Rc::new(RefCell::new(None));

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
                                            deployment_id,
                                            module,
                                            http_listeners,
                                        } => {
                                            function_deployments.borrow_mut().insert(deployment_id.clone(), Rc::new(RefCell::new(FunctionDeployment::new(&wasmtime, module).await)));
                                            // TODO: cleanup old deployments
                                            functions.borrow_mut().insert(function_id.clone(), deployment_id);

                                            {
                                                let mut http_hosts = http_hosts.borrow_mut();
                                                http_hosts.retain(|_k, v| v != &function_id);
                                                http_hosts.extend(http_listeners.iter().filter_map(|v| v.host.as_ref()).map(|v| (v.to_lowercase(), function_id.clone())));
                                            }

                                            if http_listeners.iter().find(|v| v.host.is_none()).is_some() {
                                                *http_default.borrow_mut() = Some(function_id.clone());
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
                                        .serve_connection(io, HttpHandlerV2::new(
                                            http_hosts.clone(),
                                            http_default.clone(),
                                            functions.clone(),
                                            function_deployments.clone(),
                                        ));
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

struct HttpHandlerV2 {
    http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
    http_default: Rc<RefCell<Option<FunctionId>>>,
    functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>>,
}

impl HttpHandlerV2 {
    pub fn new(
        http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
        http_default: Rc<RefCell<Option<FunctionId>>>,
        functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
        function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>>,
    ) -> Self {
        Self {
            http_hosts,
            http_default,
            functions,
            function_deployments,
        }
    }
}

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for HttpHandlerV2 {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        let target_function = req.headers().get("Host")
            .and_then(|v| self.http_hosts.borrow().get(&v.to_str().unwrap().to_lowercase()).cloned())
            .or_else(|| self.http_default.borrow().clone());
        let target_function_deployment_id = target_function.and_then(|function_id| self.functions.borrow().get(&function_id).cloned());
        let target_function_deployment = target_function_deployment_id.and_then(|instance_id| self.function_deployments.borrow().get(&instance_id).cloned());

        Box::pin(async move {
            let target_function_deployment = match target_function_deployment {
                Some(v) => v,
                None => {
                    let mut response = Response::new(Full::new(Bytes::from("no fx function found to handle this request.\n".as_bytes())));
                    *response.status_mut() = StatusCode::BAD_GATEWAY;
                    return Ok(response);
                }
            };

            let function_future = target_function_deployment.borrow().handle_request(FunctionRequest::new());
            let response = function_future.await;

            unimplemented!()
        })
    }
}

struct DefinitionsMonitor {
    functions_directory: PathBuf,
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
    compiler_tx: flume::Sender<CompilerMessage>,

    // DefinitionsMonitor requests FunctionDeployment creation and assigns IDs to them.
    deployment_id_counter: u64,
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
            deployment_id_counter: 0,
        }
    }

    async fn scan_definitions(&mut self) {
        info!("will scan definitions in: {:?}", self.functions_directory);

        let root = self.functions_directory.clone();
        for entry in WalkDir::new(&root) {
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
        watcher.watch(&root, notify::RecursiveMode::Recursive).unwrap();

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

        let deployment_id = self.next_deployment_id();
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
                deployment_id: deployment_id.clone(),
                module: module.clone(),
                http_listeners: http_listeners.clone(),
            }).await.unwrap();
        }
    }

    fn next_deployment_id(&mut self) -> FunctionDeploymentId {
        let deployment_id = FunctionDeploymentId::new(self.deployment_id_counter);
        self.deployment_id_counter += 1;
        deployment_id
    }
}

#[derive(Error, Debug)]
enum FunctionIdDetectionError {
    #[error("config path missing .fx.yaml extension")]
    PathMissingExtension,
}

/// deployment is a set of FunctionInstances deployed with same configuration
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct FunctionDeploymentId {
    id: u64,
}

impl FunctionDeploymentId {
    fn new(id: u64) -> Self {
        Self { id }
    }
}

struct FunctionDeployment {
    module: wasmtime::Module,
    instance_template: wasmtime::InstancePre<FunctionInstanceState>,
    instance: Rc<FunctionInstance>,
}

impl FunctionDeployment {
    pub async fn new(wasmtime: &wasmtime::Engine, module: wasmtime::Module) -> Self {
        let mut linker = wasmtime::Linker::<FunctionInstanceState>::new(wasmtime);
        linker.func_wrap("fx", "fx_api", fx_api_handler).unwrap();
        for import in module.imports() {
            if import.module() == "fx" {
                continue;
            }

            if let Some(f) = import.ty().func() {
                linker.func_new(
                    import.module(),
                    import.name(),
                    f.clone(),
                    move |_, _, _| {
                        Err(wasmtime::Error::msg("requested function is not implemented by fx runtime"))
                    }
                ).unwrap();
            }
        }
        let instance_template = linker.instantiate_pre(&module).unwrap();

        let instance = FunctionInstance::new(wasmtime, &instance_template).await;

        Self {
            module,
            instance_template,
            instance: Rc::new(instance),
        }
    }

    fn handle_request(&self, req: FunctionRequest) -> FunctionFuture {
        FunctionFuture::new(self.instance.clone(), req)
    }
}

struct FunctionInstance {
    instance: wasmtime::Instance,
    store: wasmtime::Store<FunctionInstanceState>,
    fn_malloc: wasmtime::TypedFunc<i64, i64>,
}

impl FunctionInstance {
    pub async fn new(wasmtime: &wasmtime::Engine, instance_template: &wasmtime::InstancePre<FunctionInstanceState>) -> Self {
        let mut store = wasmtime::Store::new(wasmtime, FunctionInstanceState::new());
        let instance = instance_template.instantiate_async(&mut store).await.unwrap();

        let fn_malloc = instance.get_typed_func::<i64, i64>(store.as_context_mut(), "_fx_malloc").unwrap();

        Self {
            instance,
            store,
            fn_malloc,
        }
    }
}

struct FunctionInstanceState {}

impl FunctionInstanceState {
    pub fn new() -> Self {
        Self {}
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

fn fx_api_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: i64, req_len: i64, output_ptr: i64) {
    unimplemented!("fx api handling is not implemented yet")
}

struct FunctionRequest {}

impl FunctionRequest {
    pub fn new() -> Self {
        Self {}
    }
}

struct FunctionResponse {}

impl FunctionResponse {
    pub fn new() -> Self {
        Self {}
    }
}

struct FunctionFuture {
    instance: Rc<FunctionInstance>,
    req: FunctionRequest,
}

impl FunctionFuture {
    pub fn new(instance: Rc<FunctionInstance>, req: FunctionRequest) -> Self {
        Self {
            instance,
            req,
        }
    }

    fn invoke_fx_api(&self, message: capnp::message::Builder<capnp::message::HeapAllocator>) {
        let response_ptr = {
            let message_size = capnp::serialize::compute_serialized_size_in_words(&message) * 8;
        };

        unimplemented!("fx api calling is not supported yet")
    }

    fn malloc(&self) {

    }
}

impl Future for FunctionFuture {
    type Output = FunctionResponse;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        unimplemented!()
    }
}
