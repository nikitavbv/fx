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
            kv::KvMessage,
            blob::BlobMessage,
        },
        effects::{
            logs::LogMessageEvent,
            metrics::FunctionMetricsDelta,
        },
        function::{FunctionDeploymentId, FunctionId, deployment::{FunctionDeployment, DeploymentInitError, FunctionDeploymentHandleRequestError}},
        triggers::http::HttpHandler,
    },
    super::{WorkerMessage, WorkerLocalMessage, LocalWorkerController, messages::FunctionInvokeError},
};

pub(crate) struct WorkerConfig {
    pub(crate) core_id: Option<usize>,
    pub(crate) port: u16,
    pub(crate) messages_rx: flume::Receiver<WorkerMessage>,
    pub(crate) sql_tx: flume::Sender<SqlMessage>,
    pub(crate) kv_tx: flume::Sender<KvMessage>,
    pub(crate) blob_tx: flume::Sender<BlobMessage>,
    pub(crate) logger_tx: flume::Sender<LogMessageEvent>,
    pub(crate) management_tx: flume::Sender<ManagementMessage>,
}

#[derive(Clone)]
struct WorkerWorld {
    wasmtime: Rc<wasmtime::Engine>,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<FunctionDeployment>>>>,
    functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
    http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
    http_default: Rc<RefCell<Option<FunctionId>>>,
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

    let addr: SocketAddr = ([0, 0, 0, 0], worker.port).into();
    socket.bind(&addr.into()).unwrap();
    socket.listen(1024).unwrap();

    // setup wasm runtime:
    let world = WorkerWorld {
        wasmtime: Rc::new(wasmtime),
        function_deployments: Rc::new(RefCell::new(HashMap::new())),
        functions: Rc::new(RefCell::new(HashMap::new())),
        http_hosts: Rc::new(RefCell::new(HashMap::new())),
        http_default: Rc::new(RefCell::new(None)),
    };

    // run worker:
    tokio_runtime.block_on(local_set.run_until(async {
        let listener = tokio::net::TcpListener::from_std(socket.into()).unwrap();
        let graceful = hyper_util::server::graceful::GracefulShutdown::new();

        let mut metrics_flush_interval = tokio::time::interval(Duration::from_secs(2));

        let (local_message_queue_tx, mut local_message_queue_rx) = async_unsync::unbounded::channel::<WorkerLocalMessage>().into_split();
        let local_controller = LocalWorkerController::new(local_message_queue_tx);

        loop {
            tokio::select! {
                message = worker.messages_rx.recv_async() => {
                    worker_handle_message(
                        &world,
                        &worker,
                        local_controller.clone(),
                        message.unwrap(),
                    ).await;
                },

                message = local_message_queue_rx.recv() => {
                    worker_handle_local_message(
                        world.function_deployments.clone(),
                        world.functions.clone(),
                        message.unwrap(),
                    ).await;
                }

                connection = listener.accept() => {
                    worker_handle_http_connection(
                        &graceful,
                        world.function_deployments.clone(),
                        world.functions.clone(),
                        world.http_hosts.clone(),
                        world.http_default.clone(),
                        connection,
                    ).await;
                },

                _ = metrics_flush_interval.tick() => {
                    worker_handle_metrics_flush(&worker, world.function_deployments.clone());
                }
            }
        }
    }));
}

async fn worker_handle_message(
    world: &WorkerWorld,
    worker: &WorkerConfig,
    local_controller: LocalWorkerController,
    message: WorkerMessage
) {
    match message {
        WorkerMessage::FunctionDeploy {
            function_id,
            deployment_id,
            module,
            limit_memory_bytes,
            http_listeners,
            env,
            bindings_sql,
            bindings_blob,
            bindings_kv,
            bindings_functions,
        } => {
            let deployment = FunctionDeployment::new(
                world.wasmtime.clone(),
                limit_memory_bytes,
                local_controller.clone(),
                worker.logger_tx.clone(),
                worker.sql_tx.clone(),
                worker.kv_tx.clone(),
                worker.blob_tx.clone(),
                function_id.clone(),
                module,
                env,
                bindings_sql,
                bindings_blob,
                bindings_kv,
                bindings_functions,
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

            world.function_deployments.borrow_mut().insert(
                deployment_id.clone(),
                Rc::new(deployment)
            );
            // TODO: cleanup old deployments
            world.functions.borrow_mut().insert(function_id.clone(), deployment_id);

            {
                let mut http_hosts = world.http_hosts.borrow_mut();
                http_hosts.retain(|_k, v| v != &function_id);
                http_hosts.extend(http_listeners.iter().filter_map(|v| v.host.as_ref()).map(|v| (v.to_lowercase(), function_id.clone())));
            }

            if http_listeners.iter().any(|v| v.host.is_none()) {
                *world.http_default.borrow_mut() = Some(function_id.clone());
            }
        },
        WorkerMessage::RemoveFunction { function_id, on_ready } => {
            world.http_hosts.borrow_mut().retain(|_k, v| v != &function_id);
            {
                let mut http_default = world.http_default.borrow_mut();
                if http_default.is_some() && http_default.as_ref().unwrap() == &function_id {
                    *http_default = None;
                }
            }

            // removing function is an idempotent operation: requesting function to be removed multiple times consecutively
            // must result in the same behavior and response as the first. That's why we silently ignore if deployment does not
            // exist instead of returning an error via on_ready.
            if let Some(deployment) = world.functions.borrow_mut().remove(&function_id) {
                world.function_deployments.borrow_mut().remove(&deployment);
            }

            if let Some(on_ready) = on_ready {
                on_ready.send(()).unwrap();
            }
        },
        WorkerMessage::FunctionInvoke { function_id, header, response_tx } => {
            let deployment = match world.functions.borrow().get(&function_id) {
                Some(v) => v.clone(),
                None => {
                    response_tx.send(Err(FunctionInvokeError::NotFound)).unwrap();
                    return;
                }
            };

            let deployment = world.function_deployments.borrow().get(&deployment).unwrap().clone();

            let function_future = deployment.handle_request(header, None).await;
            tokio::task::spawn_local(async move {
                response_tx.send(
                    function_future.await
                        .map(|_| ())
                        .map_err(|err| match err {
                            FunctionDeploymentHandleRequestError::FunctionPanicked => FunctionInvokeError::FunctionPanicked,
                        })
                ).unwrap();
            });
        },
    }
}

async fn worker_handle_local_message(
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<FunctionDeployment>>>>,
    functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
    message: WorkerLocalMessage
) {
    match message {
        WorkerLocalMessage::FunctionInvoke { function_id, header, response_tx } => {
            let deployment = function_deployments.borrow().get(functions.borrow().get(&function_id).unwrap()).unwrap().clone();
            let function_future = deployment.handle_request(header, None).await;
            tokio::task::spawn_local(async move {
                let response = function_future.await.unwrap();
                response_tx.send(response).unwrap();
            });
        }
    }
}

async fn worker_handle_http_connection(
    graceful: &hyper_util::server::graceful::GracefulShutdown,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<FunctionDeployment>>>>,
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

fn worker_handle_metrics_flush(
    worker: &WorkerConfig,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<FunctionDeployment>>>>,
) {
    // copying references to instances to avoid holding references to instances across store lock await point
    let instances = function_deployments
        .borrow()
        .values()
        .map(|deployment| (deployment.function_id.clone(), deployment.instance.clone()))
        .collect::<Vec<_>>();

    let mut function_metrics = HashMap::<FunctionId, FunctionMetricsDelta, _>::new();

    for (function_id, instance) in instances {
        let instance = instance.borrow();
        let mut store = match instance.store.try_lock() {
            Some(v) => v,
            None => continue,
        };
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
