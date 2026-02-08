use {
    std::{
        io::Cursor,
        path::{PathBuf, Path},
        collections::HashMap,
        net::SocketAddr,
        convert::Infallible,
        pin::Pin,
        rc::Rc,
        cell::RefCell,
        task::Poll,
        thread::JoinHandle,
        fmt::Debug,
        marker::PhantomData,
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
    wasmtime::{AsContext, AsContextMut},
    futures_intrusive::sync::LocalMutex,
    slotmap::{SlotMap, Key as SlotMapKey},
    fx_types::{capnp, abi_capnp, abi_function_resources_capnp, abi::FuturePollResult},
    crate::{
        common::LogMessageEvent,
        runtime::{
            runtime::{FxRuntime, FunctionId, Engine},
            kv::{BoxedStorage, FsStorage, SuffixStorage, KVStorage},
            definition::{DefinitionProvider, load_rabbitmq_consumer_task_from_config},
            metrics::run_metrics_server,
            logs::{self, BoxLogger, StdoutLogger, NoopLogger},
        },
        server::{
            server::FxServer,
            config::{ServerConfig, FunctionConfig, FunctionCodeConfig},
        },
    },
};

const FILE_EXTENSION_WASM: &str = ".wasm";
const DEFINITION_FILE_SUFFIX: &str = ".fx.yaml";

#[derive(Debug)]
enum WorkerMessage {
    RemoveFunction(FunctionId),
    FunctionDeploy {
        function_id: FunctionId,
        deployment_id: FunctionDeploymentId,
        module: wasmtime::Module,

        http_listeners: Vec<FunctionHttpListener>,
    },
}

struct DebugWrapper<T>(T);

impl<T> DebugWrapper<T> {
    fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Debug for DebugWrapper<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        "DebugWrapper<T>".fmt(f)
    }
}

impl<T> AsRef<T> for DebugWrapper<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

struct CompilerMessage {
    function_id: FunctionId,
    code: Vec<u8>,
    response: oneshot::Sender<wasmtime::Module>,
}

struct ManagementMessage {
    function_id: FunctionId,
    function_config: FunctionConfig,
    on_ready: oneshot::Sender<()>,
}

pub struct FxServerV2 {
    config: ServerConfig,
}

impl FxServerV2 {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    pub fn start(self) -> RunningFxServer {
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
        let (management_tx, management_rx) = flume::unbounded::<ManagementMessage>();

        let management_thread_handle = {
            let workers_tx = workers_tx.clone();

            std::thread::spawn(move || {
                info!("started management thread");

                let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let local_set = tokio::task::LocalSet::new();

                let mut definitions_monitor = DefinitionsMonitor::new(&self.config, workers_tx, compiler_tx);

                tokio_runtime.block_on(local_set.run_until(async {
                    tokio::join!(
                        definitions_monitor.scan_definitions(),
                        async {
                            while let Ok(msg) = management_rx.recv_async().await {
                                definitions_monitor.apply_config(msg.function_id, msg.function_config).await;
                                msg.on_ready.send(()).unwrap();
                            }
                        }
                    )
                }));
            })
        };

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
                                        function_deployments.borrow_mut().insert(
                                            deployment_id.clone(),
                                            Rc::new(RefCell::new(FunctionDeployment::new(&wasmtime, function_id.clone(), module).await))
                                        );
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

        RunningFxServer {
            worker_tx: workers_tx,
            management_tx,

            invoke_roundrobin_index: tokio::sync::Mutex::new(0),

            worker_handles,
            compiler_thread_handle,
            management_thread_handle,
        }
    }
}

pub struct RunningFxServer {
    worker_tx: Vec<flume::Sender<WorkerMessage>>,
    management_tx: flume::Sender<ManagementMessage>,

    invoke_roundrobin_index: tokio::sync::Mutex<usize>,

    worker_handles: Vec<JoinHandle<()>>,
    compiler_thread_handle: JoinHandle<()>,
    management_thread_handle: JoinHandle<()>,
}

impl RunningFxServer {
    pub async fn deploy_function(&self, function_id: FunctionId, function_config: FunctionConfig) {
        let (response_tx, response_rx) = oneshot::channel();

        self.management_tx.send_async(ManagementMessage { function_id, function_config, on_ready: response_tx }).await.unwrap();

        response_rx.await.unwrap();
    }

    pub fn wait_until_finished(self) {
        for handle in self.worker_handles {
            handle.join().unwrap();
        }
        self.compiler_thread_handle.join().unwrap();
        self.management_thread_handle.join().unwrap();
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
    type Response = Response<FunctionResponseHttpBody>;
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
                    let mut response = Response::new(FunctionResponseHttpBody::for_bytes(Bytes::from("no fx function found to handle this request.\n".as_bytes())));
                    *response.status_mut() = StatusCode::BAD_GATEWAY;
                    return Ok(response);
                }
            };

            let function_future = target_function_deployment.borrow().handle_request(FunctionRequest::from(req));
            let response = function_future.await;
            let function_response: FunctionResponse = response.move_to_host().await;

            let mut response = Response::new(FunctionResponseHttpBody::for_bytes(Bytes::from("not implemented.\n".as_bytes())));
            match function_response.0 {
                FunctionResponseInner::HttpResponse(v) => {
                    *response.status_mut() = v.status;
                }
            }

            Ok(response)
        })
    }
}

struct FunctionResponseHttpBody(FunctionResponseHttpBodyInner);

impl FunctionResponseHttpBody {
    pub fn for_bytes(bytes: Bytes) -> Self {
        Self(FunctionResponseHttpBodyInner::Full(RefCell::new(Some(bytes))))
    }
}

impl hyper::body::Body for FunctionResponseHttpBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match &self.0 {
            FunctionResponseHttpBodyInner::Full(b) => Poll::Ready(b.replace(None).map(|v| Ok(hyper::body::Frame::data(v)))),
        }
    }
}

enum FunctionResponseHttpBodyInner {
    Full(RefCell<Option<Bytes>>),
}

struct DefinitionsMonitor {
    functions_directory: PathBuf,
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
    compiler_tx: flume::Sender<CompilerMessage>,

    // DefinitionsMonitor requests FunctionDeployment creation and assigns IDs to them.
    deployment_id_counter: RefCell<u64>,
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
            deployment_id_counter: RefCell::new(0),
        }
    }

    async fn scan_definitions(&self) {
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
        if let Err(err) = watcher.watch(&root, notify::RecursiveMode::Recursive) {
            error!("failed to start watching functions directory: {err:?}");
            return;
        }

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

    async fn apply_config(&self, function_id: FunctionId, config: FunctionConfig) {
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

    fn next_deployment_id(&self) -> FunctionDeploymentId {
        let deployment_id = FunctionDeploymentId::new(*self.deployment_id_counter.borrow());
        *self.deployment_id_counter.borrow_mut() += 1;
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
    pub async fn new(wasmtime: &wasmtime::Engine, function_id: FunctionId, module: wasmtime::Module) -> Self {
        let mut linker = wasmtime::Linker::<FunctionInstanceState>::new(wasmtime);

        linker.func_wrap("fx", "fx_api", fx_api_handler).unwrap();

        linker.func_wrap("fx", "fx_log", fx_log_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_serialize", fx_resource_serialize_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_move_from_host", fx_resource_move_from_host_handler).unwrap();

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

        let instance = FunctionInstance::new(wasmtime, function_id, &instance_template).await;

        Self {
            module,
            instance_template,
            instance: Rc::new(instance),
        }
    }

    fn handle_request(&self, req: FunctionRequest) -> Pin<Box<dyn Future<Output = SerializedFunctionResource<FunctionResponse>>>> {
        let instance = self.instance.clone();

        Box::pin(async move {
            let resource = instance.store.lock().await.data_mut().resource_add(Resource::FunctionRequest(SerializableResource::Raw(req)));
            let response_resource = FunctionFuture::new(instance.clone(), instance.invoke_http_trigger(&resource).await).await;
            SerializedFunctionResource::new(instance, response_resource)
        })
    }
}

struct FunctionInstance {
    instance: wasmtime::Instance,
    store: LocalMutex<wasmtime::Store<FunctionInstanceState>>,
    memory: wasmtime::Memory,
    // fx apis:
    fn_future_poll: wasmtime::TypedFunc<u64, i64>,
    fn_resource_serialize: wasmtime::TypedFunc<u64, u64>,
    fn_resource_serialized_ptr: wasmtime::TypedFunc<u64, i64>,
    fn_resource_drop: wasmtime::TypedFunc<u64, ()>,
    // triggers:
    fn_trigger_http: wasmtime::TypedFunc<u64, u64>,
}

impl FunctionInstance {
    pub async fn new(
        wasmtime: &wasmtime::Engine,
        function_id: FunctionId,
        instance_template: &wasmtime::InstancePre<FunctionInstanceState>,
    ) -> Self {
        let mut store = wasmtime::Store::new(wasmtime, FunctionInstanceState::new(function_id));
        let instance = instance_template.instantiate_async(&mut store).await.unwrap();

        let memory = instance.get_memory(store.as_context_mut(), "memory").unwrap();

        let fn_future_poll = instance.get_typed_func::<u64, i64>(store.as_context_mut(), "_fx_future_poll").unwrap();
        let fn_resource_serialize = instance.get_typed_func::<u64, u64>(store.as_context_mut(), "_fx_resource_serialize").unwrap();
        let fn_resource_serialized_ptr = instance.get_typed_func::<u64, i64>(store.as_context_mut(), "_fx_resource_serialized_ptr").unwrap();
        let fn_resource_drop = instance.get_typed_func(store.as_context_mut(), "_fx_resource_drop").unwrap();

        let fn_trigger_http = instance.get_typed_func(store.as_context_mut(), "__fx_handler_http").unwrap();

        // We are using async calls to exported functions to enable epoch-based preemption.
        // We also allow functions to handle concurrent requests. That introduces an interesting
        // edge case: once preempted, function has to resume execution for the same future and
        // request that triggered it. You cannot just resume execution with a different function call.
        // That means that while we use call_async, we need somehow to guarantee that each function
        // call will be executed to completion before fx function does anything else.
        // Using tokio::sync::Mutex would go against the idea of having no sync between threads and atomics,
        // so given this is a single-threaded runtime, we can use LocalMutex instead.
        let store = LocalMutex::new(store, false);

        Self {
            instance,
            store,
            memory,
            fn_future_poll,
            fn_resource_serialize,
            fn_resource_serialized_ptr,
            fn_resource_drop,
            fn_trigger_http,
        }
    }

    async fn future_poll(&self, future_id: &FunctionResourceId) -> Poll<()> {
        let mut store = self.store.lock().await;
        let future_poll_result = self.fn_future_poll.call_async(store.as_context_mut(), future_id.as_u64()).await.unwrap();
        drop(store);

        match FuturePollResult::try_from(future_poll_result).unwrap() {
            FuturePollResult::Pending => Poll::Pending,
            FuturePollResult::Ready => Poll::Ready(()),
        }
    }

    async fn resource_serialize(&self, resource_id: &FunctionResourceId) -> u64 {
        let mut store = self.store.lock().await;
        self.fn_resource_serialize.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap() as u64
    }

    async fn resource_serialized_ptr(&self, resource_id: &FunctionResourceId) -> u64 {
        let mut store = self.store.lock().await;
        self.fn_resource_serialized_ptr.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap() as u64
    }

    async fn resource_drop(&self, resource_id: &FunctionResourceId) {
        let mut store = self.store.lock().await;
        self.fn_resource_drop.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap();
    }

    async fn move_serialized_resource_to_host(&self, resource_id: &FunctionResourceId) -> Vec<u8> {
        let len = self.resource_serialize(resource_id).await as usize;
        let ptr = self.resource_serialized_ptr(resource_id).await as usize;

        let resource_data = {
            let store = self.store.lock().await;
            let view = self.memory.data(store.as_context());
            view[ptr..ptr+len].to_owned()
        };

        self.resource_drop(resource_id).await;

        resource_data
    }

    async fn invoke_http_trigger(&self, resource_id: &ResourceId) -> FunctionResourceId {
        let store = self.store.lock();
        FunctionResourceId::new(self.fn_trigger_http.call_async(store.await.as_context_mut(), resource_id.as_u64()).await.unwrap() as u64)
    }
}

struct FunctionInstanceState {
    function_id: FunctionId,
    resources: SlotMap<slotmap::DefaultKey, Resource>,
}

impl FunctionInstanceState {
    pub fn new(function_id: FunctionId) -> Self {
        Self {
            function_id,
            resources: SlotMap::new(),
        }
    }

    pub fn resource_add(&mut self, resource: Resource) -> ResourceId {
        ResourceId::from(self.resources.insert(resource))
    }

    pub fn resource_serialize(&mut self, resource_id: &ResourceId) -> usize {
        let resource = self.resources.detach(resource_id.into()).unwrap();
        let (resource, serialized_size) = match resource {
            Resource::FunctionRequest(req) => {
                let serialized = req.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::FunctionRequest(serialized), serialized_size)
            },
        };
        self.resources.reattach(resource_id.into(), resource);
        serialized_size
    }

    pub fn resource_remove(&mut self, resource_id: &ResourceId) -> Resource {
        self.resources.remove(resource_id.into()).unwrap()
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
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = &view[req_addr as usize..(req_addr + req_len) as usize];
    let message_reader = fx_types::capnp::serialize::read_message_from_flat_slice(&mut message_bytes, fx_types::capnp::message::ReaderOptions::default()).unwrap();
    let request = message_reader.get_root::<fx_types::abi_capnp::fx_api_call::Reader>().unwrap();
    let op = request.get_op();

    unimplemented!("fx apis are deprecated: {:?}", op)
}

fn fx_log_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: i64, req_len: i64) {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = &view[req_addr as usize..(req_addr + req_len) as usize];
    let message_reader = fx_types::capnp::serialize::read_message_from_flat_slice(&mut message_bytes, fx_types::capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<fx_types::abi_log_capnp::log_message::Reader>().unwrap();

    let message: LogMessageEvent = logs::LogMessage::new(
        logs::LogSource::function(&caller.data().function_id),
        match message.get_event_type().unwrap() {
            fx_types::abi_log_capnp::EventType::Begin => logs::LogEventType::Begin,
            fx_types::abi_log_capnp::EventType::End => logs::LogEventType::End,
            fx_types::abi_log_capnp::EventType::Instant => logs::LogEventType::Instant,
        },
        match message.get_level().unwrap() {
            fx_types::abi_log_capnp::LogLevel::Trace => logs::LogLevel::Trace,
            fx_types::abi_log_capnp::LogLevel::Debug => logs::LogLevel::Debug,
            fx_types::abi_log_capnp::LogLevel::Info => logs::LogLevel::Info,
            fx_types::abi_log_capnp::LogLevel::Warn => logs::LogLevel::Warn,
            fx_types::abi_log_capnp::LogLevel::Error => logs::LogLevel::Error,
        },
        message.get_fields().unwrap()
            .into_iter()
            .map(|v| (v.get_name().unwrap().to_string().unwrap(), v.get_value().unwrap().to_string().unwrap()))
            .collect()
    ).into();

    // TODO: support custom loggers
    println!("{message:?}");
}

fn fx_resource_serialize_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) -> u64 {
    caller.data_mut().resource_serialize(&ResourceId::from(resource_id)) as u64
}

fn fx_resource_move_from_host_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) {
    let resource = match caller.data_mut().resource_remove(&ResourceId::from(resource_id)) {
        Resource::FunctionRequest(req) => req.into_serialized(),
    };

    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;

    view[ptr..ptr+resource.len()].copy_from_slice(&resource);
}

#[derive(Debug)]
pub struct FunctionRequest(FunctionRequestInner);

impl From<hyper::Request<hyper::body::Incoming>> for FunctionRequest {
    fn from(value: hyper::Request<hyper::body::Incoming>) -> Self {
        FunctionRequest(FunctionRequestInner::Http(value))
    }
}

impl From<hyper::Request<http_body_util::Full<hyper::body::Bytes>>> for FunctionRequest {
    fn from(value: hyper::Request<http_body_util::Full<hyper::body::Bytes>>) -> Self {
        FunctionRequest(FunctionRequestInner::HttpInline(value))
    }
}

impl FunctionRequest {
}

#[derive(Debug)]
enum FunctionRequestInner {
    Http(hyper::Request<hyper::body::Incoming>),
    HttpInline(hyper::Request<Full<Bytes>>),
}

struct FunctionResponse(FunctionResponseInner);

enum FunctionResponseInner {
    HttpResponse(FunctionHttpResponse),
}

struct FunctionHttpResponse {
    status: http::status::StatusCode,
}

struct SerializedFunctionResource<T: DeserializeFunctionResource> {
    _t: PhantomData<T>,
    instance: Rc<FunctionInstance>,
    resource: FunctionResourceId,
}

impl<T: DeserializeFunctionResource> SerializedFunctionResource<T> {
    pub fn new(instance: Rc<FunctionInstance>, resource: FunctionResourceId) -> Self {
        Self {
            _t: PhantomData,
            instance,
            resource,
        }
    }

    async fn move_to_host(self) -> T {
        T::deserialize(&mut self.instance.move_serialized_resource_to_host(&self.resource).await.as_slice())
    }
}

trait DeserializeFunctionResource {
    fn deserialize(resource: &mut &[u8]) -> Self;
}

impl DeserializeFunctionResource for FunctionResponse {
    fn deserialize(resource: &mut &[u8]) -> Self {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let response = message_reader.get_root::<abi_function_resources_capnp::function_response::Reader>().unwrap();
        Self(FunctionResponseInner::HttpResponse(FunctionHttpResponse {
            status: http::StatusCode::from_u16(response.get_status()).unwrap(),
        }))
    }
}

struct ResourceId {
    id: u64,
}

impl ResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<slotmap::DefaultKey> for ResourceId {
    fn from(value: slotmap::DefaultKey) -> Self {
        Self::new(value.data().as_ffi())
    }
}

impl Into<slotmap::DefaultKey> for &ResourceId {
    fn into(self) -> slotmap::DefaultKey {
        slotmap::DefaultKey::from(slotmap::KeyData::from_ffi(self.id))
    }
}

impl From<u64> for ResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

#[derive(Clone)]
struct FunctionResourceId {
    id: u64,
}

impl FunctionResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

enum Resource {
    FunctionRequest(SerializableResource<FunctionRequest>),
}

enum SerializableResource<T: SerializeResource> {
    Raw(T),
    Serialized(Vec<u8>),
}

impl<T: SerializeResource> SerializableResource<T> {
    fn map_to_serialized(self) -> Self {
        match self {
            Self::Raw(t) => Self::Serialized(t.serialize()),
            Self::Serialized(v) => Self::Serialized(v),
        }
    }

    fn serialized_size(&self) -> usize {
        match self {
            Self::Raw(_) => panic!("cannot compute serialized size for resource that is not serialized yet"),
            Self::Serialized(v) => v.len(),
        }
    }

    fn into_serialized(self) -> Vec<u8> {
        match self {
            Self::Raw(t) => t.serialize(),
            Self::Serialized(v) => v,
        }
    }
}

trait SerializeResource {
    fn serialize(self) -> Vec<u8>;
}

impl SerializeResource for FunctionRequest {
    fn serialize(self) -> Vec<u8> {
        unimplemented!("serialization is not implemented for function requests yet")
    }
}

struct FunctionFuture {
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
}

impl FunctionFuture {
    fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            instance,
            resource_id,
        }
    }
}

impl Future for FunctionFuture {
    type Output = FunctionResourceId;

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let future_poll_future = std::pin::pin!(self.instance.future_poll(&self.resource_id));

        match future_poll_future.poll(cx) {
            Poll::Pending | Poll::Ready(Poll::Pending) => Poll::Pending,
            Poll::Ready(Poll::Ready(_)) => Poll::Ready(self.resource_id.clone()),
        }
    }
}
