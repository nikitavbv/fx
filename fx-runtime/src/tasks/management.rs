enum ManagementMessage {
    DeployFunction(DeployFunctionMessage),
    WorkerMetrics(MetricsFlushMessage),
}

struct DeployFunctionMessage {
    function_id: FunctionId,
    function_config: FunctionConfig,
    on_ready: oneshot::Sender<()>,
}

#[derive(Debug)]
struct MetricsFlushMessage {
    function_metrics: HashMap<FunctionId, FunctionMetricsDelta>,
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
