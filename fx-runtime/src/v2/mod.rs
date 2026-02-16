use {
    std::{
        io::Cursor,
        path::{PathBuf, Path},
        collections::HashMap,
        net::SocketAddr,
        convert::Infallible,
        pin::Pin,
        rc::Rc,
        cell::{RefCell, Cell},
        task::Poll,
        thread::JoinHandle,
        fmt::Debug,
        marker::PhantomData,
        time::{Duration, SystemTime, UNIX_EPOCH},
        sync::{Arc, RwLock},
    },
    tracing::{Level, info, error, warn},
    tracing_subscriber::FmtSubscriber,
    tokio::{fs, sync::oneshot},
    clap::{Parser, Subcommand, ValueEnum, builder::PossibleValue},
    ::futures::{FutureExt, StreamExt, future::BoxFuture, future::LocalBoxFuture},
    hyper::{Response, body::Bytes, server::conn::http1, StatusCode},
    hyper_util::rt::{TokioIo, TokioTimer},
    http_body_util::{Full, BodyStream},
    walkdir::WalkDir,
    thiserror::Error,
    notify::Watcher,
    wasmtime::{AsContext, AsContextMut},
    futures_intrusive::sync::LocalMutex,
    slotmap::{SlotMap, Key as SlotMapKey},
    rand::TryRngCore,
    axum::{Router, routing::get, response::Response as AxumResponse, Extension},
    leptos::prelude::*,
    serde::Serialize,
    fx_types::{
        capnp,
        abi_capnp,
        abi_function_resources_capnp,
        abi_log_capnp,
        abi_sql_capnp,
        abi_blob_capnp,
        abi_http_capnp,
        abi_metrics_capnp,
        abi::FuturePollResult,
    },
    crate::{
        common::LogMessageEvent,
        runtime::{
            runtime::{FxRuntime, FunctionId, Engine},
            kv::{BoxedStorage, FsStorage, SuffixStorage, KVStorage},
            definition::{DefinitionProvider, load_rabbitmq_consumer_task_from_config},
            metrics::run_metrics_server,
            logs::{self, BoxLogger, StdoutLogger, NoopLogger, Logger},
            sql::{Value as SqlValue, Row as SqlRow, QueryResult},
        },
        server::{
            server::FxServer,
            config::{ServerConfig, FunctionConfig, FunctionCodeConfig, LoggerConfig},
        },
    },
    self::{
        definitions::DefinitionsMonitor,
        errors::{FunctionFuturePollError, FunctionFutureError, FunctionDeploymentHandleRequestError},
    },
};

mod definitions;
mod errors;

#[derive(Debug)]
enum WorkerMessage {
    RemoveFunction(FunctionId),
    FunctionDeploy {
        function_id: FunctionId,
        deployment_id: FunctionDeploymentId,
        module: wasmtime::Module,

        http_listeners: Vec<FunctionHttpListener>,

        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
    },
}

#[derive(Debug)]
enum SqlMessage {
    Exec(SqlExecMessage),
    Migrate(SqlMigrateMessage),
}

#[derive(Debug)]
struct SqlExecMessage {
    binding: SqlBindingConfig,
    statement: String,
    params: Vec<SqlValue>,
    response: oneshot::Sender<QueryResult>,
}

#[derive(Debug)]
struct SqlMigrateMessage {
    binding: SqlBindingConfig,
    migrations: Vec<String>,
    response: oneshot::Sender<()>,
}

#[derive(Debug)]
struct MetricsFlushMessage {
    function_metrics: HashMap<FunctionId, FunctionMetricsDelta>,
}

#[derive(Debug)]
struct FunctionMetricsDelta {
    counters_delta: HashMap<MetricKey, u64>,
}

impl FunctionMetricsDelta {
    pub fn empty() -> Self {
        Self {
            counters_delta: HashMap::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.counters_delta.is_empty()
    }

    fn append(&mut self, other: FunctionMetricsDelta) {
        for (metric_key, delta) in other.counters_delta {
            *self.counters_delta.entry(metric_key).or_insert(0) += delta;
        }
    }
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

enum ManagementMessage {
    DeployFunction(DeployFunctionMessage),
    WorkerMetrics(MetricsFlushMessage),
}

struct DeployFunctionMessage {
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
        let sql_threads = 4.min(cpu_info.map(|v| v.num_logical_cores()).unwrap_or(usize::MAX));

        let (workers_tx, workers_rx) = (0..worker_threads)
            .map(|_| flume::unbounded::<WorkerMessage>())
            .unzip::<_, _, Vec<_>, Vec<_>>();
        let (sql_tx, sql_rx) = flume::unbounded::<SqlMessage>();
        let (compiler_tx, compiler_rx) = flume::unbounded::<CompilerMessage>();
        let (management_tx, management_rx) = flume::unbounded::<ManagementMessage>();
        let (logger_tx, logger_rx) = flume::unbounded::<LogMessageEvent>();

        let management_thread_handle = {
            let config = self.config.clone();
            let workers_tx = workers_tx.clone();

            std::thread::spawn(move || {
                info!("started management thread");

                let tokio_runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let local_set = tokio::task::LocalSet::new();

                let definitions_monitor = DefinitionsMonitor::new(&config, workers_tx, compiler_tx);
                let metrics = Arc::new(MetricsRegistry::new());

                tokio_runtime.block_on(local_set.run_until(async {
                    tokio::join!(
                        definitions_monitor.scan_definitions(),
                        async {
                            while let Ok(msg) = management_rx.recv_async().await {
                                match msg {
                                    ManagementMessage::DeployFunction(msg) => {
                                        definitions_monitor.apply_config(msg.function_id, msg.function_config).await;
                                        msg.on_ready.send(()).unwrap();
                                    },
                                    ManagementMessage::WorkerMetrics(msg) => {
                                        metrics.update(msg.function_metrics);
                                    },
                                }
                            }
                        },
                        async {
                            run_introspection_server(metrics.clone()).await;
                        },
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

        let logger_thread_handle = {
            let logger_config = self.config.logger.clone().unwrap_or(LoggerConfig::Stdout);

            std::thread::spawn(move || {
                info!("started logger thread");

                let logger = create_logger(&logger_config);

                while let Ok(msg) = logger_rx.recv() {
                    logger.log(msg);
                }
            })
        };

        let sql_cores = cpu_info.as_ref()
            .map(|v| v.logical_processor_ids().iter().take(sql_threads).map(|v| Some(*v)).collect::<Vec<_>>())
            .unwrap_or(std::iter::repeat(None).take(sql_threads).collect());

        let mut sql_worker_id = 0;
        let mut sql_worker_handles = Vec::new();
        for sql_worker in sql_cores.into_iter() {
            let sql_rx = sql_rx.clone();

            let handle = std::thread::spawn(move || {
                use rusqlite::types::ValueRef;

                let worker = sql_worker;

                if let Some(worker_core_id) = worker {
                    match gdt_cpus::pin_thread_to_core(worker_core_id) {
                        Ok(_) => {},
                        Err(gdt_cpus::Error::Unsupported(_)) => {},
                        Err(err) => error!("failed to pin sql worker thread to core: {err:?}"),
                    }
                }

                info!(sql_worker_id, "started sql thread");

                let mut connections = HashMap::<String, rusqlite::Connection>::new();

                while let Ok(msg) = sql_rx.recv() {
                    let binding = match &msg {
                        SqlMessage::Exec(v) => &v.binding,
                        SqlMessage::Migrate(v) => &v.binding,
                    };

                    let connection = connections.entry(binding.connection_id.clone())
                        .or_insert_with(|| match &binding.location {
                            SqlBindingConfigLocation::InMemory(v) => rusqlite::Connection::open_with_flags(
                                format!("file:{v}"),
                                rusqlite::OpenFlags::default()
                                    .union(rusqlite::OpenFlags::SQLITE_OPEN_MEMORY)
                                    .union(rusqlite::OpenFlags::SQLITE_OPEN_SHARED_CACHE)
                            ).unwrap(),
                            SqlBindingConfigLocation::Path(v) => rusqlite::Connection::open(v).unwrap(),
                        });

                    match msg {
                        SqlMessage::Exec(msg) => {
                            let mut stmt = connection.prepare(&msg.statement).unwrap();
                            let result_columns = stmt.column_count();

                            let mut rows = stmt.query(rusqlite::params_from_iter(msg.params.into_iter())).unwrap();

                            let mut result_rows = Vec::new();

                            while let Some(row) = rows.next().unwrap() {
                                let mut row_columns = Vec::new();
                                for column in 0..result_columns {
                                    let column = row.get_ref(column).unwrap();

                                    row_columns.push(match column {
                                        ValueRef::Null => SqlValue::Null,
                                        ValueRef::Integer(v) => SqlValue::Integer(v),
                                        ValueRef::Real(v) => SqlValue::Real(v),
                                        ValueRef::Text(v) => SqlValue::Text(
                                            String::from_utf8(v.to_owned()).unwrap()
                                        ),
                                        ValueRef::Blob(v) => SqlValue::Blob(v.to_owned()),
                                    });
                                }
                                result_rows.push(SqlRow { columns: row_columns });
                            }

                            msg.response.send(QueryResult { rows: result_rows }).unwrap();
                        },
                        SqlMessage::Migrate(msg) => {
                            let mut rusqlite_migrations = Vec::new();
                            for migration in &msg.migrations {
                                rusqlite_migrations.push(rusqlite_migration::M::up(migration));
                            }

                            let migrations = rusqlite_migration::Migrations::new(rusqlite_migrations);

                            migrations.to_latest(connection).unwrap();

                            msg.response.send(()).unwrap();
                        },
                    }
                }
            });
            sql_worker_handles.push(handle);
            sql_worker_id += 1;
        }

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
            let sql_tx = sql_tx.clone();
            let logger_tx = logger_tx.clone();
            let management_tx = management_tx.clone();

            let handle = std::thread::spawn(move || {
                let worker = worker;

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

                    let mut metrics_flush_interval = tokio::time::interval(Duration::from_secs(2));

                    loop {
                        tokio::select! {
                            message = worker.messages_rx.recv_async() => {
                                match message.unwrap() {
                                    WorkerMessage::FunctionDeploy {
                                        function_id,
                                        deployment_id,
                                        module,
                                        http_listeners,
                                        bindings_sql,
                                        bindings_blob,
                                    } => {
                                        let deployment = FunctionDeployment::new(
                                            &wasmtime,
                                            logger_tx.clone(),
                                            sql_tx.clone(),
                                            function_id.clone(),
                                            module,
                                            bindings_sql,
                                            bindings_blob,
                                        ).await;
                                        let deployment = match deployment {
                                            Ok(v) => v,
                                            Err(err) => {
                                                match err {
                                                    // TODO: report issues with user function somewhere
                                                    DeploymentInitError::MissingImport => warn!(function_id=function_id.as_str(), "failed to deploy because of function requested import that fx runtime does not provide"),
                                                    DeploymentInitError::MissingExport => warn!(function_id=function_id.as_str(), "failed to deploy because function does not provide export that fx runtime expects"),
                                                };
                                                continue;
                                            },
                                        };

                                        function_deployments.borrow_mut().insert(
                                            deployment_id.clone(),
                                            Rc::new(RefCell::new(deployment))
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
                            },

                            _ = metrics_flush_interval.tick() => {
                                // copying references to instances to avoid holding references to instances across store lock await point
                                let instances = function_deployments
                                    .borrow()
                                    .values()
                                    .map(|deployment| {
                                        let deployment = deployment.borrow();
                                        (deployment.function_id.clone(), deployment.instance.clone())
                                    })
                                    .collect::<Vec<_>>();

                                let mut function_metrics = HashMap::<FunctionId, FunctionMetricsDelta, _>::new();

                                for (function_id, instance) in instances {
                                    let mut store = instance.store.lock().await;
                                    let state = store.data_mut();

                                    let metrics_delta = state.metrics.flush_delta();
                                    if metrics_delta.is_empty() {
                                        continue;
                                    }

                                    if let Some(metrics) = function_metrics.get_mut(&function_id) {
                                        metrics.append(metrics_delta);
                                    } else {
                                        function_metrics.insert(function_id, metrics_delta);
                                    }
                                }

                                if !function_metrics.is_empty() {
                                    management_tx.send(ManagementMessage::WorkerMetrics(MetricsFlushMessage { function_metrics })).unwrap();
                                }
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

            worker_handles,
            sql_worker_handles,
            compiler_thread_handle,
            management_thread_handle,
            logger_thread_handle,
        }
    }
}

pub struct RunningFxServer {
    worker_tx: Vec<flume::Sender<WorkerMessage>>,
    management_tx: flume::Sender<ManagementMessage>,

    worker_handles: Vec<JoinHandle<()>>,
    sql_worker_handles: Vec<JoinHandle<()>>,
    compiler_thread_handle: JoinHandle<()>,
    management_thread_handle: JoinHandle<()>,
    logger_thread_handle: JoinHandle<()>,
}

impl RunningFxServer {
    #[allow(dead_code)]
    pub async fn deploy_function(&self, function_id: FunctionId, function_config: FunctionConfig) {
        let (response_tx, response_rx) = oneshot::channel();

        self.management_tx.send_async(ManagementMessage::DeployFunction(DeployFunctionMessage { function_id, function_config, on_ready: response_tx })).await.unwrap();

        response_rx.await.unwrap();
    }

    pub fn wait_until_finished(self) {
        for handle in self.worker_handles {
            handle.join().unwrap();
        }
        for handle in self.sql_worker_handles {
            handle.join().unwrap();
        }
        self.compiler_thread_handle.join().unwrap();
        self.logger_thread_handle.join().unwrap();
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

            let (header, body) = req.into_parts();

            let function_future = target_function_deployment.borrow().handle_request(FetchRequestHeader::from(header), FetchRequestBody::from(body));
            let response = function_future.await;
            let function_response = match response {
                Ok(v) => Ok(v.move_to_host().await),
                Err(err) => Err(err),
            };

            let body = match &function_response {
                Ok(response) => match &response.0 {
                    FunctionResponseInner::HttpResponse(v) => {
                        FunctionResponseHttpBody::for_function_resource(v.body.replace(None).unwrap())
                    }
                },
                Err(err) => match err {
                    FunctionDeploymentHandleRequestError::FunctionPanicked => FunctionResponseHttpBody::for_bytes(Bytes::from("function panicked while handling request.\n"))
                }
            };

            let mut response = Response::new(body);
            match function_response {
                Ok(function_response) => match &function_response.0 {
                    FunctionResponseInner::HttpResponse(v) => {
                        *response.status_mut() = v.status;
                    }
                },
                Err(err) => match err {
                    FunctionDeploymentHandleRequestError::FunctionPanicked => {
                        *response.status_mut() = StatusCode::BAD_GATEWAY;
                    }
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

    pub fn for_function_resource(resource: SerializedFunctionResource<Vec<u8>>) -> Self {
        Self(FunctionResponseHttpBodyInner::FunctionResource(RefCell::new(FunctionResourceReader::Resource(resource))))
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
            FunctionResponseHttpBodyInner::Full(b) => return Poll::Ready(b.replace(None).map(|v| Ok(hyper::body::Frame::data(v)))),
            FunctionResponseHttpBodyInner::FunctionResource(resource) => {
                let reader = resource.replace(FunctionResourceReader::Empty);

                let mut reader = match reader {
                    FunctionResourceReader::Empty => return Poll::Ready(None),
                    FunctionResourceReader::Future(v) => FunctionResourceReader::Future(v),
                    FunctionResourceReader::Resource(v) => {
                        FunctionResourceReader::Future(async move {
                            v.move_to_host().await
                        }.boxed_local())
                    }
                };

                let poll_result = match &mut reader {
                    FunctionResourceReader::Empty | FunctionResourceReader::Resource(_) => unreachable!(),
                    FunctionResourceReader::Future(v) => v.poll_unpin(cx).map(|v| Some(Ok(hyper::body::Frame::data(Bytes::from(v))))),
                };

                if poll_result.is_pending() {
                    resource.replace(reader);
                }

                poll_result
            },
        }
    }
}

enum FunctionResponseHttpBodyInner {
    Full(RefCell<Option<Bytes>>),
    FunctionResource(RefCell<FunctionResourceReader>),
}

enum FunctionResourceReader {
    Empty,
    Resource(SerializedFunctionResource<Vec<u8>>),
    Future(LocalBoxFuture<'static, Vec<u8>>),
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
    function_id: FunctionId,
    module: wasmtime::Module,
    instance_template: wasmtime::InstancePre<FunctionInstanceState>,
    instance: Rc<FunctionInstance>,
}

impl FunctionDeployment {
    pub async fn new(
        wasmtime: &wasmtime::Engine,
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        function_id: FunctionId,
        module: wasmtime::Module,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
    ) -> Result<Self, DeploymentInitError> {
        let mut linker = wasmtime::Linker::<FunctionInstanceState>::new(wasmtime);

        linker.func_wrap("fx", "fx_api", fx_api_handler).unwrap();

        linker.func_wrap("fx", "fx_log", fx_log_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_serialize", fx_resource_serialize_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_move_from_host", fx_resource_move_from_host_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_drop", fx_resource_drop_handler).unwrap();
        linker.func_wrap("fx", "fx_sql_exec", fx_sql_exec_handler).unwrap();
        linker.func_wrap("fx", "fx_sql_migrate", fx_sql_migrate_handler).unwrap();
        linker.func_wrap("fx", "fx_future_poll", fx_future_poll_handler).unwrap();
        linker.func_wrap("fx", "fx_sleep", fx_sleep_handler).unwrap();
        linker.func_wrap("fx", "fx_random", fx_random_handler).unwrap();
        linker.func_wrap("fx", "fx_time", fx_time_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_put", fx_blob_put_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_get", fx_blob_get_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_delete", fx_blob_delete_handler).unwrap();
        linker.func_wrap("fx", "fx_fetch", fx_fetch_handler).unwrap();
        linker.func_wrap("fx", "fx_metrics_counter_register", fx_metrics_counter_register_handler).unwrap();
        linker.func_wrap("fx", "fx_metrics_counter_increment", fx_metrics_counter_increment_handler).unwrap();
        linker.func_wrap("fx", "fx_stream_frame_read", fx_stream_frame_read_handler).unwrap();

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

        let instance_template = linker.instantiate_pre(&module)
            .map_err(|err| {
                if let Some(_) = err.downcast_ref::<wasmtime::UnknownImportError>() {
                    return DeploymentInitError::MissingImport;
                } else {
                    todo!("handle other error: {err:?}")
                }
            })?;

        let instance = FunctionInstance::new(wasmtime, logger_tx, sql_tx, function_id.clone(), &instance_template, bindings_sql, bindings_blob).await
            .map_err(|err| match err {
                FunctionInstanceInitError::MissingExport => DeploymentInitError::MissingExport,
            })?;

        Ok(Self {
            function_id,
            module,
            instance_template,
            instance: Rc::new(instance),
        })
    }

    fn handle_request(&self, header: FetchRequestHeader, body: FetchRequestBody) -> Pin<Box<dyn Future<Output = Result<SerializedFunctionResource<FunctionResponse>, FunctionDeploymentHandleRequestError>>>> {
        let instance = self.instance.clone();

        Box::pin(async move {
            let mut header = header;
            let resource = {
                let mut data = instance.store.lock().await;
                let data = data.data_mut();
                header.body_resource_id = Some(data.resource_add(Resource::RequestBody(body)));
                data.resource_add(Resource::FetchRequest(SerializableResource::Raw(header)))
            };

            FunctionFuture::new(instance.clone(), instance.invoke_http_trigger(&resource).await).await
                .map(|response_resource| SerializedFunctionResource::new(instance, response_resource))
                .map_err(|err| match err {
                    FunctionFutureError::FunctionPanicked => FunctionDeploymentHandleRequestError::FunctionPanicked,
                })
        })
    }
}

#[derive(Debug, Error)]
enum DeploymentInitError {
    #[error("function requested import that fx runtime does not provide")]
    MissingImport,
    #[error("function does not provide export that fx runtime expects")]
    MissingExport,
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
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        function_id: FunctionId,
        instance_template: &wasmtime::InstancePre<FunctionInstanceState>,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
    ) -> Result<Self, FunctionInstanceInitError> {
        let mut store = wasmtime::Store::new(wasmtime, FunctionInstanceState::new(logger_tx, sql_tx, function_id, bindings_sql, bindings_blob));
        let instance = instance_template.instantiate_async(&mut store).await.unwrap();

        let memory = instance.get_memory(store.as_context_mut(), "memory").unwrap();

        let fn_future_poll = instance.get_typed_func::<u64, i64>(store.as_context_mut(), "_fx_future_poll")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_resource_serialize = instance.get_typed_func::<u64, u64>(store.as_context_mut(), "_fx_resource_serialize")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_resource_serialized_ptr = instance.get_typed_func::<u64, i64>(store.as_context_mut(), "_fx_resource_serialized_ptr")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_resource_drop = instance.get_typed_func(store.as_context_mut(), "_fx_resource_drop")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;

        let fn_trigger_http = instance.get_typed_func(store.as_context_mut(), "__fx_handler_http")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;

        // We are using async calls to exported functions to enable epoch-based preemption.
        // We also allow functions to handle concurrent requests. That introduces an interesting
        // edge case: once preempted, function has to resume execution for the same future and
        // request that triggered it. You cannot just resume execution with a different function call.
        // That means that while we use call_async, we need somehow to guarantee that each function
        // call will be executed to completion before fx function does anything else.
        // Using tokio::sync::Mutex would go against the idea of having no sync between threads and atomics,
        // so given this is a single-threaded runtime, we can use LocalMutex instead.
        let store = LocalMutex::new(store, false);

        Ok(Self {
            instance,
            store,
            memory,
            fn_future_poll,
            fn_resource_serialize,
            fn_resource_serialized_ptr,
            fn_resource_drop,
            fn_trigger_http,
        })
    }

    async fn future_poll(&self, future_id: &FunctionResourceId, waker: std::task::Waker) -> Result<Poll<()>, FunctionFuturePollError> {
        let mut store = self.store.lock().await;
        store.data_mut().waker = Some(waker);
        let future_poll_result = self.fn_future_poll.call_async(store.as_context_mut(), future_id.as_u64()).await;
        drop(store);

        let future_poll_result = future_poll_result.map_err(|err| {
            let trap = err.downcast::<wasmtime::Trap>().unwrap();
            match trap {
                wasmtime::Trap::UnreachableCodeReached => FunctionFuturePollError::FunctionPanicked,
                other => panic!("unexpected trap: {other:?}"),
            }
        })?;

        Ok(match FuturePollResult::try_from(future_poll_result).unwrap() {
            FuturePollResult::Pending => Poll::Pending,
            FuturePollResult::Ready => Poll::Ready(()),
        })
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

    async fn move_serializable_resource_to_host(&self, resource_id: &FunctionResourceId) -> Vec<u8> {
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

#[derive(Debug, Error)]
enum FunctionInstanceInitError {
    #[error("function does not provide export that fx runtime expects to be present")]
    MissingExport,
}

struct FunctionInstanceState {
    waker: Option<std::task::Waker>,
    logger_tx: flume::Sender<LogMessageEvent>,
    sql_tx: flume::Sender<SqlMessage>,
    function_id: FunctionId,
    resources: SlotMap<slotmap::DefaultKey, Resource>,
    bindings_sql: HashMap<String, SqlBindingConfig>,
    bindings_blob: HashMap<String, BlobBindingConfig>,
    http_client: reqwest::Client,
    metrics: FunctionMetricsState,
}

impl FunctionInstanceState {
    pub fn new(
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        function_id: FunctionId,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
    ) -> Self {
        Self {
            waker: None,
            logger_tx,
            sql_tx,
            function_id,
            resources: SlotMap::new(),
            bindings_sql,
            bindings_blob,
            http_client: reqwest::Client::new(),
            metrics: FunctionMetricsState::new(),
        }
    }

    pub fn resource_add(&mut self, resource: Resource) -> ResourceId {
        ResourceId::from(self.resources.insert(resource))
    }

    pub fn resource_serialize(&mut self, resource_id: &ResourceId) -> usize {
        let resource = self.resources.detach(resource_id.into()).unwrap();
        let (resource, serialized_size) = match resource {
            Resource::FetchRequest(req) => {
                let serialized = req.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::FetchRequest(serialized), serialized_size)
            },
            Resource::SqlQueryResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::SqlQueryResult(FutureResource::Ready(serialized)), serialized_size)
            },
            Resource::UnitFuture(_) => panic!("unit future cannot be serialized"),
            Resource::BlobGetResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::BlobGetResult(FutureResource::Ready(serialized)), serialized_size)
            },
            Resource::FetchResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::FetchResult(FutureResource::Ready(serialized)), serialized_size)
            },
            Resource::RequestBody(v) => match v.0 {
                FetchRequestBodyInner::Full(v) => {
                    let serialized = serialize_request_body_full(v);
                    let serialized_size = serialized.len();
                    (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::FullSerialized(serialized))), serialized_size)
                },
                FetchRequestBodyInner::FullSerialized(serialized) => {
                    let serialized_size = serialized.len();
                    (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::FullSerialized(serialized))), serialized_size)
                },
                FetchRequestBodyInner::Stream(_) => panic!("resource is not yet ready for serialization"),
                FetchRequestBodyInner::PartiallyReadStream { stream, frame } => {
                    let frame_serialized = serialize_partially_read_stream(frame);
                    let serialized_size = frame_serialized.len();
                    (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStreamSerialized { frame_serialized, stream })), serialized_size)
                },
                FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized } => {
                    let serialized_size = frame_serialized.len();
                    (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStreamSerialized { frame_serialized, stream })), serialized_size)
                }
            },
        };
        self.resources.reattach(resource_id.into(), resource);
        serialized_size
    }

    pub fn resource_poll(&mut self, resource_id: &ResourceId) -> Poll<()> {
        let resource = self.resources.detach(resource_id.into()).unwrap();

        let mut cx = std::task::Context::from_waker(self.waker.as_ref().unwrap());
        let (resource, poll_result) = match resource {
            Resource::FetchRequest(v) => (Resource::FetchRequest(v), Poll::Ready(())),
            Resource::SqlQueryResult(v) => {
                let (resource, poll_result) = match v {
                    FutureResource::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                    FutureResource::Future(mut future) => {
                        let poll_result = future.poll_unpin(&mut cx);
                        match poll_result {
                            Poll::Pending => (FutureResource::Future(future), Poll::Pending),
                            Poll::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                        }
                    }
                };
                (Resource::SqlQueryResult(resource), poll_result)
            },
            Resource::UnitFuture(mut v) => {
                let poll_result = v.poll_unpin(&mut cx);
                (Resource::UnitFuture(v), poll_result)
            },
            Resource::BlobGetResult(v) => {
                let (resource, poll_result) = match v {
                    FutureResource::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                    FutureResource::Future(mut future) => {
                        let poll_result = future.poll_unpin(&mut cx);
                        match poll_result {
                            Poll::Pending => (FutureResource::Future(future), Poll::Pending),
                            Poll::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                        }
                    }
                };
                (Resource::BlobGetResult(resource), poll_result)
            },
            Resource::FetchResult(v) => {
                let (resource, poll_result) = match v {
                    FutureResource::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                    FutureResource::Future(mut future) => {
                        let poll_result = future.poll_unpin(&mut cx);
                        match poll_result {
                            Poll::Pending => (FutureResource::Future(future), Poll::Pending),
                            Poll::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                        }
                    }
                };
                (Resource::FetchResult(resource), poll_result)
            },
            Resource::RequestBody(v) => match v.0 {
                FetchRequestBodyInner::Full(v) => (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::Full(v))), Poll::Ready(())),
                FetchRequestBodyInner::FullSerialized(v) => (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::FullSerialized(v))), Poll::Ready(())),
                FetchRequestBodyInner::Stream(mut stream) => {
                    use hyper::body::Body;

                    let poll_result = stream.as_mut().poll_frame(&mut cx);

                    match poll_result {
                        Poll::Pending => (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::Stream(stream))), Poll::Pending),
                        Poll::Ready(frame) => (
                            Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStream {
                                frame,
                                stream,
                            })),
                            Poll::Ready(()),
                        ),
                    }
                },
                FetchRequestBodyInner::PartiallyReadStream { stream, frame } => (
                    Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStream { stream, frame })),
                    Poll::Ready(()),
                ),
                FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized } => (
                    Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized })),
                    Poll::Ready(())
                ),
            },
        };

        self.resources.reattach(resource_id.into(), resource);

        poll_result
    }

    pub fn resource_remove(&mut self, resource_id: &ResourceId) -> Resource {
        self.resources.remove(resource_id.into()).unwrap()
    }

    pub(crate) fn stream_read_frame(&mut self, resource_id: &ResourceId) -> Vec<u8> {
        let resource = self.resources.detach(resource_id.into()).unwrap();

        let (resource, serialized_frame) = match resource {
            Resource::BlobGetResult(_)
            | Resource::FetchRequest(_)
            | Resource::FetchResult(_)
            | Resource::SqlQueryResult(_)
            | Resource::UnitFuture(_) => panic!("resource of this type does not support reading frames"),
            Resource::RequestBody(v) => match v.0 {
                FetchRequestBodyInner::Full(_)
                | FetchRequestBodyInner::Stream(_)
                | FetchRequestBodyInner::PartiallyReadStream { .. } => panic!("request body has to be serialized first"),
                FetchRequestBodyInner::FullSerialized(v) => (None, v),
                FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized } => (Some(Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::Stream(stream)))), frame_serialized),
            },
        };

        if let Some(resource) = resource {
            self.resources.reattach(resource_id.into(), resource);
        } else {
            self.resources.remove(resource_id.into());
        }

        serialized_frame
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
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<abi_log_capnp::log_message::Reader>().unwrap();

    let message: LogMessageEvent = logs::LogMessage::new(
        logs::LogSource::function(&caller.data().function_id),
        match message.get_event_type().unwrap() {
            abi_log_capnp::EventType::Begin => logs::LogEventType::Begin,
            abi_log_capnp::EventType::End => logs::LogEventType::End,
            abi_log_capnp::EventType::Instant => logs::LogEventType::Instant,
        },
        match message.get_level().unwrap() {
            abi_log_capnp::LogLevel::Trace => logs::LogLevel::Trace,
            abi_log_capnp::LogLevel::Debug => logs::LogLevel::Debug,
            abi_log_capnp::LogLevel::Info => logs::LogLevel::Info,
            abi_log_capnp::LogLevel::Warn => logs::LogLevel::Warn,
            abi_log_capnp::LogLevel::Error => logs::LogLevel::Error,
        },
        message.get_fields().unwrap()
            .into_iter()
            .map(|v| (v.get_name().unwrap().to_string().unwrap(), v.get_value().unwrap().to_string().unwrap()))
            .collect()
    ).into();

    caller.data().logger_tx.send(message).unwrap();
}

fn fx_resource_serialize_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) -> u64 {
    caller.data_mut().resource_serialize(&ResourceId::from(resource_id)) as u64
}

fn fx_resource_move_from_host_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) {
    let resource = match caller.data_mut().resource_remove(&ResourceId::from(resource_id)) {
        Resource::FetchRequest(req) => req.into_serialized(),
        Resource::SqlQueryResult(req) => match req {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::UnitFuture(_) => panic!("unit future cannot be moved to function"),
        Resource::BlobGetResult(res) => match res {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::FetchResult(res) => match res {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::RequestBody(_) => panic!("resource of this type cannot be moved"),
    };

    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;

    view[ptr..ptr+resource.len()].copy_from_slice(&resource);
}

fn fx_resource_drop_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) {
    let _ = caller.data_mut().resource_remove(&ResourceId::from(resource_id));
}

fn fx_stream_frame_read_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) {
    let serialized_frame = caller.data_mut().stream_read_frame(&ResourceId::from(resource_id));

    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;

    view[ptr..ptr+serialized_frame.len()].copy_from_slice(&serialized_frame);
}

fn fx_sql_exec_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: u64, req_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = {
        let ptr = req_addr as usize;
        let len = req_len as usize;
        &view[ptr..ptr+len]
    };
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<abi_sql_capnp::sql_exec_request::Reader>().unwrap();

    let binding = caller.data().bindings_sql.get(message.get_binding().unwrap().to_str().unwrap()).unwrap();

    let (response_tx, response_rx) = oneshot::channel();
    caller.data().sql_tx.send(SqlMessage::Exec(SqlExecMessage {
        binding: binding.clone(),
        statement: message.get_statement().unwrap().to_string().unwrap(),
        params: message.get_params().unwrap().into_iter()
            .map(|v| match v.get_value().which().unwrap() {
                abi_sql_capnp::sql_value::value::Null(_) => SqlValue::Null,
                abi_sql_capnp::sql_value::value::Integer(v) => SqlValue::Integer(v),
                abi_sql_capnp::sql_value::value::Real(v) => SqlValue::Real(v),
                abi_sql_capnp::sql_value::value::Which::Text(v) => SqlValue::Text(v.unwrap().to_string().unwrap()),
                abi_sql_capnp::sql_value::value::Which::Blob(v) => SqlValue::Blob(v.unwrap().to_vec()),
            })
            .collect(),
        response: response_tx,
    })).unwrap();

    caller.data_mut().resource_add(Resource::SqlQueryResult(FutureResource::Future(async move {
        SerializableResource::Raw(response_rx.await.unwrap())
    }.boxed()))).as_u64()
}

fn fx_sql_migrate_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: u64, req_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = {
        let ptr = req_addr as usize;
        let len = req_len as usize;

        &view[ptr..ptr+len]
    };
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<abi_sql_capnp::sql_migrate_request::Reader>().unwrap();

    let binding = caller.data().bindings_sql.get(message.get_binding().unwrap().to_str().unwrap()).unwrap();

    let (response_tx, response_rx) = oneshot::channel();
    caller.data().sql_tx.send(SqlMessage::Migrate(SqlMigrateMessage {
        binding: binding.clone(),
        migrations: message.get_migrations().unwrap().into_iter()
            .map(|v| v.unwrap().to_string().unwrap())
            .collect(),
        response: response_tx,
    })).unwrap();

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        response_rx.await.unwrap();
    }.boxed())).as_u64()
}

fn fx_future_poll_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, future_resource_id: u64) -> i64 {
    let resource_id = ResourceId::new(future_resource_id);
    (match caller.data_mut().resource_poll(&resource_id) {
        Poll::Pending => FuturePollResult::Pending,
        Poll::Ready(_) => FuturePollResult::Ready,
    }) as i64
}

fn fx_sleep_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, sleep_millis: u64) -> u64 {
    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        tokio::time::sleep(Duration::from_millis(sleep_millis)).await;
    }.boxed())).as_u64()
}

fn fx_random_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, ptr: u64, len: u64) {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;
    let len = len as usize;

    rand::rngs::OsRng.try_fill_bytes(&mut view[ptr..ptr+len]).unwrap();
}

fn fx_time_handler(_caller: wasmtime::Caller<'_, FunctionInstanceState>) -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn fx_blob_put_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    binding_ptr: u64,
    binding_len: u64,
    key_ptr: u64,
    key_len: u64,
    value_ptr: u64,
    value_len: u64
) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_ptr as usize;
        let len = binding_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };

    let binding = caller.data().bindings_blob.get(&binding).unwrap();

    let key = {
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };
    // TODO: add a check to prevent exiting the directory, lol
    let key_path = binding.storage_directory.join(key);

    let value = {
        let ptr = value_ptr as usize;
        let len = value_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        let parent = key_path.parent().unwrap();
        if !parent.exists() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }

        tokio::fs::write(key_path, value).await.unwrap();
    }.boxed())).as_u64()
}

fn fx_blob_get_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    binding_ptr: u64,
    binding_len: u64,
    key_ptr: u64,
    key_len: u64,
) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_ptr as usize;
        let len = binding_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };
    let binding = caller.data().bindings_blob.get(&binding);

    let key_path = binding.map(|v| v.storage_directory.join({
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    }));

    caller.data_mut().resource_add(Resource::BlobGetResult(FutureResource::Future(async move {
        SerializableResource::Raw({
            match key_path {
                Some(v) => match tokio::fs::read(v).await {
                    Ok(v) => BlobGetResponse::Ok(v),
                    Err(err) => {
                        if err.kind() == tokio::io::ErrorKind::NotFound {
                            BlobGetResponse::NotFound
                        } else {
                            todo!("handling for this error kind is not implemented: {err:?}");
                        }
                    }
                },
                None => BlobGetResponse::BindingNotExists
            }
        })
    }.boxed()))).as_u64()
}

fn fx_blob_delete_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    binding_ptr: u64,
    binding_len: u64,
    key_ptr: u64,
    key_len: u64,
) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_ptr as usize;
        let len = binding_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };
    let binding = caller.data().bindings_blob.get(&binding).unwrap();

    let key = {
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };
    let key_path = binding.storage_directory.join(key);

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        if let Err(err) = tokio::fs::remove_file(&key_path).await {
            if err.kind() != std::io::ErrorKind::NotFound {
                todo!("error handling is not implemented for fx_blob_delete: {:?}", err.kind());
            }
        }
    }.boxed())).as_u64()
}

fn fx_fetch_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    req_ptr: u64,
    req_len: u64,
) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut request = {
        let ptr = req_ptr as usize;
        let len = req_len as usize;
        &view[ptr..ptr+len]
    };

    let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
    let request = request_reader.get_root::<abi_http_capnp::http_request::Reader>().unwrap();

    let mut fetch_request = reqwest::Request::new(
        match request.get_method().unwrap() {
            abi_http_capnp::HttpMethod::Get => http::Method::GET,
            abi_http_capnp::HttpMethod::Put => http::Method::PUT,
            abi_http_capnp::HttpMethod::Post => http::Method::POST,
            abi_http_capnp::HttpMethod::Patch => http::Method::PATCH,
            abi_http_capnp::HttpMethod::Delete => http::Method::DELETE,
            abi_http_capnp::HttpMethod::Options => http::Method::OPTIONS,
        },
        request.get_uri().unwrap().to_str().unwrap().try_into().unwrap()
    );

    match request.get_body().unwrap().get_body().which().unwrap() {
        abi_http_capnp::http_request_body::body::Which::Empty(_) => {},
        abi_http_capnp::http_request_body::body::Which::Bytes(v) => {
            *fetch_request.body_mut() = Some(reqwest::Body::from(v.unwrap().to_vec()));
        },
        abi_http_capnp::http_request_body::body::Which::HostResource(v) => {
            let resource_id = ResourceId::new(v);
            match caller.data_mut().resource_remove(&resource_id) {
                Resource::BlobGetResult(_)
                | Resource::FetchRequest(_)
                | Resource::FetchResult(_)
                | Resource::SqlQueryResult(_)
                | Resource::UnitFuture(_) => panic!("this resource cannot be used as request body"),
                Resource::RequestBody(v) => match v.0 {
                    FetchRequestBodyInner::FullSerialized(_)
                    | FetchRequestBodyInner::PartiallyReadStream { .. }
                    | FetchRequestBodyInner::PartiallyReadStreamSerialized { .. } => panic!("this body type cannot be used as request body"),
                    FetchRequestBodyInner::Full(bytes) => {
                        *fetch_request.body_mut() = Some(reqwest::Body::from(bytes));
                    },
                    FetchRequestBodyInner::Stream(stream) => {
                        let body_stream = BodyStream::new(stream)
                            .filter_map(|result| async {
                                match result {
                                    Ok(frame) => frame.into_data().ok().map(Ok),
                                    Err(e) => Some(Err(e)),
                                }
                            });
                        *fetch_request.body_mut() = Some(reqwest::Body::wrap_stream(body_stream));
                    }
                }
            }
        },
    }

    let client = caller.data().http_client.clone();

    caller.data_mut().resource_add(Resource::FetchResult(FutureResource::Future(async move {
        SerializableResource::Raw({
            let result = client.execute(fetch_request).await.unwrap();
            FetchResult::new(result.status(), result.bytes().await.unwrap().to_vec())
        })
    }.boxed()))).as_u64()
}

fn fx_metrics_counter_register_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_ptr: u64, req_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut request = {
        let ptr = req_ptr as usize;
        let len = req_len as usize;
        &view[ptr..ptr+len]
    };

    let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
    let request = request_reader.get_root::<abi_metrics_capnp::counter_register::Reader>().unwrap();

    let metric_key = MetricKey {
        name: request.get_name().unwrap().to_string().unwrap(),
        labels: {
            let mut labels = request.get_labels().unwrap().into_iter()
                .map(|v| (
                    v.get_name().unwrap().to_string().unwrap(),
                    v.get_value().unwrap().to_string().unwrap()
                ))
                .collect::<Vec<_>>();

            labels.sort();

            labels
        },
    };

    caller.data_mut().metrics.counter_register(metric_key).into_abi()
}

fn fx_metrics_counter_increment_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, counter_id: u64, delta: u64) {
    caller.data_mut().metrics.counter_increment(MetricId::new(counter_id), delta);
}

pub struct FetchRequestHeader {
    inner: http::request::Parts,
    body_resource_id: Option<ResourceId>, // TODO: drop body if FetchRequestHeader is dropped without consumption
}

impl FetchRequestHeader {
    fn uri(&self) -> &http::Uri {
        &self.inner.uri
    }

    fn method(&self) -> &http::Method {
        &self.inner.method
    }

    fn headers(&self) -> &http::HeaderMap {
        &self.inner.headers
    }
}

impl From<http::request::Parts> for FetchRequestHeader {
    fn from(value: http::request::Parts) -> Self {
        Self {
            inner: value,
            body_resource_id: None,
        }
    }
}

pub struct FetchRequestBody(FetchRequestBodyInner);

impl From<hyper::body::Incoming> for FetchRequestBody {
    fn from(value: hyper::body::Incoming) -> Self {
        Self(FetchRequestBodyInner::Stream(Box::pin(value)))
    }
}

enum FetchRequestBodyInner {
    // full body:
    Full(Vec<u8>),
    FullSerialized(Vec<u8>),
    // streaming body:
    Stream(Pin<Box<hyper::body::Incoming>>),
    PartiallyReadStream {
        stream: Pin<Box<hyper::body::Incoming>>,
        frame: Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>,
    },
    PartiallyReadStreamSerialized {
        stream: Pin<Box<hyper::body::Incoming>>,
        frame_serialized: Vec<u8>,
    },
}

struct FunctionResponse(FunctionResponseInner);

enum FunctionResponseInner {
    HttpResponse(FunctionHttpResponse),
}

struct FunctionHttpResponse {
    status: http::status::StatusCode,
    body: Cell<Option<SerializedFunctionResource<Vec<u8>>>>,
}

/// Resource that origins from function side and is not owned by host.
/// moved lazily from function to host memory.
/// if dropped before being moved, cleans up resource on function side.
struct SerializedFunctionResource<T: DeserializeFunctionResource> {
    _t: PhantomData<T>,
    resource: OwnedFunctionResourceId,
}

impl<T: DeserializeFunctionResource> SerializedFunctionResource<T> {
    pub fn new(instance: Rc<FunctionInstance>, resource: FunctionResourceId) -> Self {
        Self {
            _t: PhantomData,
            resource: OwnedFunctionResourceId::new(instance, resource),
        }
    }

    async fn move_to_host(self) -> T {
        let (instance, resource) = self.resource.consume();
        T::deserialize(&mut instance.move_serializable_resource_to_host(&resource).await.as_slice(), instance)
    }
}

/// Function resource handle that is owned by host.
/// Cleans up function memory if dropped before being consumed
pub struct OwnedFunctionResourceId(Cell<Option<(Rc<FunctionInstance>, FunctionResourceId)>>);

impl OwnedFunctionResourceId {
    pub fn new(function_instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self(Cell::new(Some((function_instance, resource_id))))
    }

    pub fn consume(self) -> (Rc<FunctionInstance>, FunctionResourceId) {
        self.0.replace(None).unwrap()
    }
}

impl Drop for OwnedFunctionResourceId {
    fn drop(&mut self) {
        if let Some((function_instance, resource_id)) = self.0.replace(None) {
            tokio::task::spawn_local(async move {
                function_instance.resource_drop(&resource_id).await;
            });
        }
    }
}

trait DeserializeFunctionResource {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self;
}

impl DeserializeFunctionResource for FunctionResponse {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let response = message_reader.get_root::<abi_function_resources_capnp::function_response::Reader>().unwrap();
        Self(FunctionResponseInner::HttpResponse(FunctionHttpResponse {
            status: http::StatusCode::from_u16(response.get_status()).unwrap(),
            body: Cell::new(Some(SerializedFunctionResource::new(instance, FunctionResourceId::from(response.get_body_resource())))),
        }))
    }
}

impl DeserializeFunctionResource for Vec<u8> {
    fn deserialize(resource: &mut &[u8], _instance: Rc<FunctionInstance>) -> Self {
        resource.to_vec()
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

#[derive(Clone, Debug)]
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

impl From<u64> for FunctionResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

enum Resource {
    FetchRequest(SerializableResource<FetchRequestHeader>),
    RequestBody(FetchRequestBody),
    SqlQueryResult(FutureResource<SerializableResource<QueryResult>>),
    UnitFuture(BoxFuture<'static, ()>),
    BlobGetResult(FutureResource<SerializableResource<BlobGetResponse>>),
    FetchResult(FutureResource<SerializableResource<FetchResult>>),
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

impl SerializeResource for FetchRequestHeader {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut resource = message.init_root::<abi_http_capnp::http_request::Builder>();

        resource.set_uri(self.uri().to_string());
        resource.set_method(match &*self.method() {
            &hyper::Method::GET => abi_http_capnp::HttpMethod::Get,
            &hyper::Method::POST => abi_http_capnp::HttpMethod::Post,
            &hyper::Method::PUT => abi_http_capnp::HttpMethod::Put,
            &hyper::Method::PATCH => abi_http_capnp::HttpMethod::Patch,
            &hyper::Method::DELETE => abi_http_capnp::HttpMethod::Delete,
            &hyper::Method::OPTIONS => abi_http_capnp::HttpMethod::Options,
            other => todo!("this http method not supported: {other:?}"),
        });

        let mut request_headers = resource.reborrow().init_headers(self.headers().len() as u32);
        for (index, (header_name, header_value)) in self.headers().iter().enumerate() {
            let mut request_header = request_headers.reborrow().get(index as u32);
            request_header.set_name(header_name.as_str());
            request_header.set_value(header_value.to_str().unwrap());
        }

        let mut resource_body = resource.init_body().init_body();
        match self.body_resource_id {
            None => resource_body.set_empty(()),
            Some(resource_id) => resource_body.set_host_resource(resource_id.as_u64()),
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

impl SerializeResource for QueryResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let sql_exec_response = message.init_root::<abi_sql_capnp::sql_exec_result::Builder>();

        let mut response_rows = sql_exec_response.init_rows(self.rows.len() as u32);
        for (index, result_row) in self.rows.into_iter().enumerate() {
            let mut response_row_columns = response_rows.reborrow().get(index as u32).init_columns(result_row.columns.len() as u32);
            for (column_index, value) in result_row.columns.into_iter().enumerate() {
                let mut response_value = response_row_columns.reborrow().get(column_index as u32).init_value();
                match value {
                    crate::runtime::sql::Value::Null => response_value.set_null(()),
                    crate::runtime::sql::Value::Integer(v) => response_value.set_integer(v),
                    crate::runtime::sql::Value::Real(v) => response_value.set_real(v),
                    crate::runtime::sql::Value::Text(v) => response_value.set_text(v),
                    crate::runtime::sql::Value::Blob(v) => response_value.set_blob(&v),
                }
            }
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

impl SerializeResource for Vec<u8> {
    fn serialize(self) -> Vec<u8> {
        self
    }
}

enum FutureResource<T> {
    Future(BoxFuture<'static, T>),
    Ready(T),
}

struct FunctionFuture {
    inner: LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>>,
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
}

impl FunctionFuture {
    fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: Self::start_new_poll_call(instance.clone(), resource_id.clone()),
            instance,
            resource_id,
        }
    }

    fn start_new_poll_call(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>> {
        async move {
            let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
            instance.future_poll(&resource_id, waker).await
        }.boxed_local()
    }
}

impl Future for FunctionFuture {
    type Output = Result<FunctionResourceId, FunctionFutureError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        match self.inner.poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(v) => {
                let poll = match v {
                    Ok(v) => v,
                    Err(err) => return Poll::Ready(Err(match err {
                        FunctionFuturePollError::FunctionPanicked => FunctionFutureError::FunctionPanicked,
                    })),
                };

                match poll {
                    Poll::Pending => {
                        self.inner = Self::start_new_poll_call(self.instance.clone(), self.resource_id.clone());
                        Poll::Pending
                    },
                    Poll::Ready(_) => Poll::Ready(Ok(self.resource_id.clone()))
                }
            },
        }
    }
}

enum BlobGetResponse {
    NotFound,
    Ok(Vec<u8>),
    BindingNotExists,
}

impl SerializeResource for BlobGetResponse {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let blob_get_response = message.init_root::<abi_blob_capnp::blob_get_response::Builder>();
        let mut response = blob_get_response.init_response();

        match self {
            Self::NotFound => response.set_not_found(()),
            Self::Ok(v) => response.set_value(&v),
            Self::BindingNotExists => response.set_binding_not_exists(()),
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

struct FetchResult {
    status: http::StatusCode,
    body: Vec<u8>,
}

impl FetchResult {
    pub fn new(status: http::StatusCode, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
        }
    }
}

impl SerializeResource for FetchResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut fetch_response = message.init_root::<abi_http_capnp::http_response::Builder>();

        fetch_response.set_status(self.status.as_u16());
        fetch_response.set_body(&self.body);

        capnp::serialize::write_message_to_words(&message)
    }
}

fn serialize_request_body_full(body: Vec<u8>) -> Vec<u8> {
    todo!("serialize request body full")
}

fn serialize_partially_read_stream(frame: Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    let serialized_frame = message.init_root::<abi_http_capnp::http_request_body_frame::Builder>();
    let mut serialized_frame = serialized_frame.init_body();

    match frame {
        None => serialized_frame.set_stream_end(()),
        Some(Err(err)) => todo!("handle error: {err:?}"),
        Some(Ok(frame)) => serialized_frame.set_bytes(&frame.into_data().unwrap()),
    }

    capnp::serialize::write_message_to_words(&message)
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionHttpListener {
    pub(crate) host: Option<String>,
}

impl FunctionHttpListener {
    fn new(host: Option<String>) -> Self {
        Self {
            host,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SqlBindingConfig {
    pub(crate) connection_id: String,
    pub(crate) location: SqlBindingConfigLocation,
}

#[derive(Debug, Clone)]
enum SqlBindingConfigLocation {
    InMemory(String),
    Path(PathBuf),
}

#[derive(Debug, Clone)]
struct BlobBindingConfig {
    storage_directory: PathBuf,
}

fn create_logger(logger: &LoggerConfig) -> BoxLogger {
    match logger {
        LoggerConfig::Stdout => BoxLogger::new(StdoutLogger::new()),
        LoggerConfig::Noop => BoxLogger::new(NoopLogger::new()),
        LoggerConfig::HttpLogger { endpoint } => BoxLogger::new(HttpLogger::new(endpoint.clone())),
        LoggerConfig::Custom(v) => BoxLogger::new(v.clone()),
    }
}

struct HttpLogger {
    client: reqwest::blocking::Client,
    endpoint: String,
}

impl HttpLogger {
    pub fn new(endpoint: String) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            endpoint,
        }
    }
}

impl Logger for HttpLogger {
    fn log(&self, message: LogMessageEvent) {
        let response = self.client.post(&self.endpoint)
            .header("content-type", "application/stream+json")
            .body(serde_json::to_vec(&HttpLogEvent {
                source: "fx".to_owned(),
                function_id: match message.source {
                    crate::common::LogSource::FxRuntime => None,
                    crate::common::LogSource::Function { id } => Some(id),
                },
                message: message.fields.get("message")
                    .map(|v| match v {
                        crate::common::EventFieldValue::Text(v) => v.clone(),
                        other => format!("{other:?}"),
                    })
                    .unwrap_or(format!("fx log")),
                request_id: message.fields.get("request_id")
                    .map(|v| match v {
                        crate::common::EventFieldValue::Text(v) => v.clone(),
                        other => format!("{other:?}"),
                    }),
                level: message.level,
                timestamp: chrono::Utc::now().to_rfc3339(),
                fields: message.fields.into_iter()
                    .map(|(key, value)| (
                        key.clone(),
                        map_log_value_to_serde_json(&value)
                    ))
                    .collect()
            }).unwrap())
            .send()
            .unwrap();
        if !response.status().is_success() {
            error!(status_code=response.status().as_u16(), "failed to send logs over http");
        }
    }
}

fn map_log_value_to_serde_json(value: &crate::common::EventFieldValue) -> serde_json::Value {
    match value {
        crate::common::EventFieldValue::Text(v) => serde_json::Value::String(v.clone()),
        crate::common::EventFieldValue::F64(v) => serde_json::Value::Number(serde_json::Number::from_f64(*v).unwrap()),
        crate::common::EventFieldValue::I64(v) => serde_json::Value::Number((*v).into()),
        crate::common::EventFieldValue::U64(v) => serde_json::Value::Number((*v).into()),
        crate::common::EventFieldValue::Object(v) => serde_json::Value::Object(
            v.iter()
                .map(|(key, value)| (key.clone(), map_log_value_to_serde_json(value)))
                .collect()
        )
    }
}

#[derive(Serialize)]
struct HttpLogEvent {
    fields: HashMap<String, serde_json::Value>,
    level: crate::common::LogEventLevel,
    message: String,
    request_id: Option<String>,
    source: String,
    function_id: Option<String>,
    timestamp: String,
}

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
struct MetricKey {
    name: String,
    labels: Vec<(String, String)>,
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct MetricId {
    id: u64,
}

impl MetricId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn from_abi(id: u64) -> Self {
        Self::new(id)
    }

    pub fn into_abi(&self) -> u64 {
        self.id
    }
}

struct FunctionMetricsState {
    metrics_series: Vec<MetricKey>,
    metric_key_to_ids: HashMap<MetricKey, MetricId>,
    metrics_counter_delta: HashMap<MetricId, u64>,
}

impl FunctionMetricsState {
    pub fn new() -> Self {
        Self {
            metrics_series: Vec::new(),
            metric_key_to_ids: HashMap::new(),
            metrics_counter_delta: HashMap::new(),
        }
    }

    fn counter_register(&mut self, key: MetricKey) -> MetricId {
        *self.metric_key_to_ids.entry(key.clone()).or_insert_with(|| {
            let index = self.metrics_series.len() as u64;
            self.metrics_series.push(key);
            MetricId::new(index)
        })
    }

    fn counter_increment(&mut self, key: MetricId, delta: u64) {
        *self.metrics_counter_delta.entry(key).or_insert(0) += delta;
    }

    fn flush_delta(&mut self) -> FunctionMetricsDelta {
        let counters_delta = std::mem::replace(&mut self.metrics_counter_delta, HashMap::new())
            .into_iter()
            .map(|(metric_id, delta)| (self.metrics_series.get(metric_id.id as usize).unwrap().clone(), delta))
            .collect();

        FunctionMetricsDelta {
            counters_delta,
        }
    }
}

struct MetricsRegistry {
    function_metrics: RwLock<HashMap<FunctionId, FunctionMetricsDelta>>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            function_metrics: RwLock::new(HashMap::new()),
        }
    }

    pub fn update(&self, function_metrics: HashMap<FunctionId, FunctionMetricsDelta>) {
        let mut all_metrics = self.function_metrics.write().unwrap();

        for (function_id, metrics) in function_metrics {
            all_metrics.entry(function_id)
                .or_insert_with(|| FunctionMetricsDelta::empty())
                .append(metrics);
        }
    }

    pub fn encode(&self) -> String {
        let mut result = String::new();
        let all_metrics = self.function_metrics.read().unwrap();

        for (function_id, function_metrics) in all_metrics.iter() {
            let function_id = sanitize_metric_name(function_id.as_str());

            for (counter_key, counter_value) in function_metrics.counters_delta.iter() {
                let metric_name = {
                    let mut metric_name = String::new();
                    metric_name.push_str("function_");
                    metric_name.push_str(sanitize_metric_name(function_id.as_str()).as_str());
                    metric_name.push('_');
                    metric_name.push_str(sanitize_metric_name(counter_key.name.as_str()).as_str());
                    metric_name
                };

                result.push_str("# TYPE ");
                result.push_str(metric_name.as_str());
                result.push_str(" counter\n");
                result.push_str(metric_name.as_str());

                if !counter_key.labels.is_empty() {
                    result.push('{');

                    for (index, (label_key, label_value)) in counter_key.labels.iter().enumerate() {
                        if index > 0 {
                            result.push(',');
                        }
                        result.push_str(sanitize_label_name(label_key.as_str()).as_str());
                        result.push_str("=\"");
                        result.push_str(escape_label_value(label_value.as_str()).as_str());
                        result.push('"');
                    }

                    result.push('}')
                }

                result.push(' ');
                result.push_str(counter_value.to_string().as_str());
                result.push('\n');
            }
        }

        result
    }
}

fn sanitize_metric_name(s: &str) -> String {
    s.chars()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                c
            } else if i == 0 && c.is_ascii_digit() {
                '_'
            } else {
                '_'
            }
        })
        .collect()
}

fn sanitize_label_name(s: &str) -> String {
    s.chars()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_alphabetic() || c == '_' || (i > 0 && c.is_ascii_digit()) { c } else { '_' }
        })
        .collect()
}

fn escape_label_value(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace('"', "\\\"")
     .replace('\n', "\\n")
}

async fn run_introspection_server(metrics: Arc<MetricsRegistry>) {
    let app = Router::new()
        .route("/", get(introspection_home))
        .route("/metrics", get(introspection_metrics))
        .layer(Extension(metrics));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn introspection_home() -> AxumResponse {
    render_component(view! {
        <>
            <h2>"fx runtime"</h2>
            <br></br>
            <a href="/introspection">"/introspection"</a>" - realtime dashboard for troubleshooting and insights."<br></br>
            <a href="/metrics">"/metrics"</a>" - metrics exported in prometheus format."<br></br>
        </>
    })
}

async fn introspection_metrics(Extension(metrics): Extension<Arc<MetricsRegistry>>) -> String {
    metrics.encode()
}

fn render_component(component: impl IntoView + 'static) -> AxumResponse {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(component.to_html().into())
        .unwrap()
}
