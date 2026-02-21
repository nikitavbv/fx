use {
    std::{sync::Arc, collections::HashMap, net::SocketAddr, rc::Rc, cell::RefCell, thread::JoinHandle},
    tracing::{error, info, warn},
    tokio::{time::Duration, sync::oneshot},
    hyper::server::conn::http1,
    hyper_util::rt::{TokioIo, TokioTimer},
    crate::{
        v2::{
            function::{FunctionDeployment, FunctionDeploymentId, DeploymentInitError},
            http::HttpHandlerV2,
            config::FunctionConfig,
            ServerConfig,
            WorkerMessage,
            SqlMessage,
            CompilerMessage,
            ManagementMessage,
            LogMessageEvent,
            definitions::DefinitionsMonitor,
            MetricsRegistry,
            run_introspection_server,
            WorkersController,
            LoggerConfig,
            create_logger,
            SqlBindingConfigLocation,
            SqlQueryExecutionError,
            SqlQueryResult,
            SqlValue,
            SqlMigrationResult,
            SqlRow,
            SqlMigrationError,
            FunctionId,
            FunctionMetricsDelta,
            MetricsFlushMessage,
            DeployFunctionMessage,
            cron::{run_cron_task, CronDatabase},
            sql::SqlDatabase,
            logs::Logger,
        },
    },
};

// TODO:
// - rate limiting - use governor crate and have a set of rate limits defined in FunctionDefinition
// - permissions - based on capabilities

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

                run_management_task(config, workers_tx, compiler_tx, management_rx);
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

        let cron_thread_handle = {
            let cron_database = match self.config.cron_data_path {
                Some(cron_data_path) => SqlDatabase::new(self.config.config_path.unwrap().parent().unwrap().join(cron_data_path)).unwrap(),
                None => SqlDatabase::in_memory().unwrap(),
            };
            let cron_database = CronDatabase::new(cron_database);

            std::thread::spawn(move || {
                info!("started cron thread");
                run_cron_task(cron_database);
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
                let worker = sql_worker;

                if let Some(worker_core_id) = worker {
                    match gdt_cpus::pin_thread_to_core(worker_core_id) {
                        Ok(_) => {},
                        Err(gdt_cpus::Error::Unsupported(_)) => {},
                        Err(err) => error!("failed to pin sql worker thread to core: {err:?}"),
                    }
                }

                info!(sql_worker_id, "started sql thread");

                run_sql_task(sql_rx);
            });
            sql_worker_handles.push(handle);
            sql_worker_id += 1;
        }

        let worker_cores = cpu_info.as_ref()
            .map(|v| v.logical_processor_ids().iter().take(worker_threads).map(|v| Some(*v)).collect::<Vec<_>>())
            .unwrap_or(std::iter::repeat(None).take(worker_threads).collect());

        let workers = worker_cores.into_iter()
            .zip(workers_rx.into_iter())
            .map(|(core_id, messages_rx)| WorkerConfig {
                core_id,
                messages_rx,
                sql_tx: sql_tx.clone(),
                logger_tx: logger_tx.clone(),
                management_tx: management_tx.clone(),
            })
            .collect::<Vec<_>>();

        let mut worker_id = 0;
        let mut worker_handles = Vec::new();
        for worker in workers.into_iter() {
            let wasmtime = wasmtime.clone();

            let handle = std::thread::spawn(move || {
                if let Some(worker_core_id) = worker.core_id {
                    match gdt_cpus::pin_thread_to_core(worker_core_id) {
                        Ok(_) => {},
                        Err(gdt_cpus::Error::Unsupported(_)) => {},
                        Err(err) => error!("failed to pin thread to core: {err:?}"),
                    }
                }

                info!(worker_id, "started worker thread");

                run_worker_task(worker, wasmtime);
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
            cron_thread_handle,
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
    cron_thread_handle: JoinHandle<()>,
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
        self.cron_thread_handle.join().unwrap();
        self.compiler_thread_handle.join().unwrap();
        self.logger_thread_handle.join().unwrap();
        self.management_thread_handle.join().unwrap();
    }
}

fn run_management_task(
    config: ServerConfig,
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
    compiler_tx: flume::Sender<CompilerMessage>,
    management_rx: flume::Receiver<ManagementMessage>,
) {
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local_set = tokio::task::LocalSet::new();

    let definitions_monitor = DefinitionsMonitor::new(&config, workers_tx.clone(), compiler_tx);
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
                run_introspection_server(metrics.clone(), WorkersController::new(workers_tx)).await;
            },
        )
    }));
}

fn run_sql_task(sql_rx: flume::Receiver<SqlMessage>) {
    use rusqlite::types::ValueRef;

    let mut connections = HashMap::<String, rusqlite::Connection>::new();

    while let Ok(msg) = sql_rx.recv() {
        let binding = match &msg {
            SqlMessage::Exec(v) => &v.binding,
            SqlMessage::Migrate(v) => &v.binding,
        };

        let connection = match connections.entry(binding.connection_id.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let connection = match &binding.location {
                    SqlBindingConfigLocation::InMemory(v) => rusqlite::Connection::open_with_flags(
                        format!("file:{v}"),
                        rusqlite::OpenFlags::default()
                            .union(rusqlite::OpenFlags::SQLITE_OPEN_MEMORY)
                            .union(rusqlite::OpenFlags::SQLITE_OPEN_SHARED_CACHE)
                    ).unwrap(),
                    SqlBindingConfigLocation::Path(v) => rusqlite::Connection::open(v).unwrap(),
                };
                if let Some(busy_timeout) = binding.busy_timeout {
                    connection.busy_timeout(busy_timeout).unwrap();
                }
                if let Err(err) = connection.pragma_update(None, "journal_mode", "WAL") {
                    if err.sqlite_error().unwrap().code == rusqlite::ErrorCode::DatabaseBusy {
                        Err(SqlQueryExecutionError::DatabaseBusy)
                    } else {
                        panic!("unexpected sqlite error: {err:?}")
                    }
                } else {
                    connection.pragma_update(None, "synchronous", "NORMAL").unwrap();
                    Ok(entry.insert(connection))
                }
            }
        };

        match msg {
            SqlMessage::Exec(msg) => {
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlQueryExecutionError::DatabaseBusy => {
                            msg.response.send(SqlQueryResult::Error(SqlQueryExecutionError::DatabaseBusy)).unwrap();
                            continue;
                        }
                    }
                };

                let mut stmt = connection.prepare(&msg.statement).unwrap();
                let result_columns = stmt.column_count();

                let mut rows = stmt.query(rusqlite::params_from_iter(msg.params.into_iter())).unwrap();

                let mut result_rows = Vec::new();
                let response_message = loop {
                    let row = rows.next();
                    let row = match row {
                        Ok(v) => v,
                        Err(err) => {
                            if err.sqlite_error().unwrap().code == rusqlite::ErrorCode::DatabaseBusy {
                                break SqlQueryResult::Error(SqlQueryExecutionError::DatabaseBusy);
                            } else {
                                panic!("unexpected sqlite error: {err:?}")
                            }
                        }
                    };
                    let row = match row {
                        Some(v) => v,
                        None => break SqlQueryResult::Ok(result_rows),
                    };

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
                };

                msg.response.send(response_message).unwrap();
            },
            SqlMessage::Migrate(msg) => {
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlQueryExecutionError::DatabaseBusy => {
                            msg.response.send(SqlMigrationResult::Error(SqlMigrationError::DatabaseBusy)).unwrap();
                            continue;
                        }
                    }
                };

                let mut rusqlite_migrations = Vec::new();
                for migration in &msg.migrations {
                    rusqlite_migrations.push(rusqlite_migration::M::up(migration));
                }

                let migrations = rusqlite_migration::Migrations::new(rusqlite_migrations);

                let response = match migrations.to_latest(connection) {
                    Ok(_) => SqlMigrationResult::Ok(()),
                    Err(err) => match err {
                        rusqlite_migration::Error::RusqliteError { query: _, err } => if err.sqlite_error().unwrap().code == rusqlite::ErrorCode::DatabaseBusy {
                            SqlMigrationResult::Error(SqlMigrationError::DatabaseBusy)
                        } else {
                            panic!("unexpected sqlite error: {err:?}");
                        },
                        other => panic!("unexpected sqlite error: {other:?}"),
                    }
                };

                msg.response.send(response).unwrap();
            },
        }
    }
}

struct WorkerConfig {
    core_id: Option<usize>,
    messages_rx: flume::Receiver<WorkerMessage>,
    sql_tx: flume::Sender<SqlMessage>,
    logger_tx: flume::Sender<LogMessageEvent>,
    management_tx: flume::Sender<ManagementMessage>,
}

fn run_worker_task(worker: WorkerConfig, wasmtime: wasmtime::Engine) {
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
                    worker_handle_message(
                        &wasmtime,
                        &worker,
                        function_deployments.clone(),
                        functions.clone(),
                        http_hosts.clone(),
                        http_default.clone(),
                        message.unwrap()
                    ).await;
                },

                connection = listener.accept() => {
                    worker_handle_http_connection(
                        &graceful,
                        function_deployments.clone(),
                        functions.clone(),
                        http_hosts.clone(),
                        http_default.clone(),
                        connection
                    ).await;
                },

                _ = metrics_flush_interval.tick() => {
                    worker_handle_metrics_flush(&worker, function_deployments.clone()).await;
                }
            }
        }
    }));
}

async fn worker_handle_message(
    wasmtime: &wasmtime::Engine,
    worker: &WorkerConfig,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>>,
    functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
    http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
    http_default: Rc<RefCell<Option<FunctionId>>>,
    message: WorkerMessage
) {
    match message {
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
                worker.logger_tx.clone(),
                worker.sql_tx.clone(),
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
                    return;
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
        WorkerMessage::RemoveFunction { function_id, on_ready } => {
            http_hosts.borrow_mut().retain(|_k, v| v != &function_id);
            {
                let mut http_default = http_default.borrow_mut();
                if http_default.is_some() && http_default.as_ref().unwrap() == &function_id {
                    *http_default = None;
                }
            }

            let deployment = functions.borrow_mut().remove(&function_id).unwrap();
            function_deployments.borrow_mut().remove(&deployment);

            if let Some(on_ready) = on_ready {
                on_ready.send(()).unwrap();
            }
        },
    }
}

async fn worker_handle_http_connection(
    graceful: &hyper_util::server::graceful::GracefulShutdown,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>>,
    functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
    http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
    http_default: Rc<RefCell<Option<FunctionId>>>,
    connection: Result<(tokio::net::TcpStream, SocketAddr), tokio::io::Error>,
) {
    let (tcp, _) = match connection {
        Ok(v) => v,
        Err(err) => {
            error!("failed to accept http connection: {err:?}");
            return;
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
}

async fn worker_handle_metrics_flush(
    worker: &WorkerConfig,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>>,
) {
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
        // TODO: handle this
        worker.management_tx.send(ManagementMessage::WorkerMetrics(MetricsFlushMessage { function_metrics })).unwrap();
    }
}
