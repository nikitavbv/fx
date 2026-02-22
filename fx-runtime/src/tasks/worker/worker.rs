use {
    std::{net::SocketAddr, rc::Rc, collections::HashMap, cell::RefCell},
    tokio::time::Duration,
    tracing::{warn, error},
    hyper_util::rt::{TokioIo, TokioTimer},
    hyper::server::conn::http1,
    crate::{
        tasks::{
            sql::SqlMessage,
            management::{ManagementMessage, MetricsFlushMessage},
        },
        effects::{
            logs::LogMessageEvent,
            metrics::FunctionMetricsDelta,
        },
        function::{FunctionDeploymentId, FunctionId, deployment::{FunctionDeployment, DeploymentInitError}},
        triggers::http::HttpHandler,
    },
    super::WorkerMessage,
};

pub(crate) struct WorkerConfig {
    pub(crate) core_id: Option<usize>,
    pub(crate) messages_rx: flume::Receiver<WorkerMessage>,
    pub(crate) sql_tx: flume::Sender<SqlMessage>,
    pub(crate) logger_tx: flume::Sender<LogMessageEvent>,
    pub(crate) management_tx: flume::Sender<ManagementMessage>,
}

pub(crate) fn run_worker_task(worker: WorkerConfig, wasmtime: wasmtime::Engine) {
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
        .serve_connection(io, HttpHandler::new(
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
