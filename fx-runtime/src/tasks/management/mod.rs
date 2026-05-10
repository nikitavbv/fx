use {
    std::{collections::HashMap, sync::Arc},
    send_wrapper::SendWrapper,
    tokio::sync::oneshot,
    tracing::info,
    crate::{
        function::FunctionId,
        definitions::{config::{FunctionConfig, ServerConfig}, monitor::DefinitionsMonitor},
        effects::metrics::{FunctionMetricsDelta, MetricsRegistry},
        tasks::{
            worker::{WorkerMessage, WorkersController},
            compiler::CompilerMessage,
            cron::{CronMessage, CronTaskEvent},
        },
        introspection::run_introspection_server,
    },
    self::runtime_state::RuntimeState,
};

pub(crate) mod runtime_state;

pub(crate) enum ManagementMessage {
    DeployFunction(DeployFunctionMessage),
    WorkerMetrics(MetricsFlushMessage),
}

pub(crate) struct DeployFunctionMessage {
    pub(crate) function_id: FunctionId,
    pub(crate) function_config: FunctionConfig,
    pub(crate) on_ready: oneshot::Sender<()>,
}

#[derive(Debug)]
pub(crate) struct MetricsFlushMessage {
    pub(crate) function_metrics: HashMap<FunctionId, FunctionMetricsDelta>,
}

pub(crate) fn run_management_task(
    config: ServerConfig,
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
    compiler_tx: flume::Sender<CompilerMessage>,
    cron_tx: flume::Sender<CronMessage>,
    cron_event_rx: flume::Receiver<CronTaskEvent>,
    management_rx: flume::Receiver<ManagementMessage>,
) {
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local_set = tokio::task::LocalSet::new();

    let runtime_state = RuntimeState::new();
    let definitions_monitor = DefinitionsMonitor::new(&config, workers_tx.clone(), compiler_tx, cron_tx.clone(), runtime_state.clone());
    let metrics = Arc::new(MetricsRegistry::new());

    let introspection_enabled = config.introspection.map(|v| v.enabled).unwrap_or(true);

    tokio_runtime.block_on(local_set.run_until(async {
        tokio::join!(
            definitions_monitor.scan_definitions(),
            async {
                loop {
                    tokio::select! {
                        msg = management_rx.recv_async() => {
                            let msg = match msg {
                                Ok(v) => v,
                                Err(_) => {
                                    info!("stopping management task, because management channel is dropped");
                                    break;
                                },
                            };
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
                        event = cron_event_rx.recv_async() => {
                            let event = match event {
                                Ok(v) => v,
                                Err(_) => {
                                    info!("stopping management task, because cron event channel is dropped");
                                    break;
                                },
                            };
                            match event {
                                CronTaskEvent::Run { name, function_id, run_at } => {
                                    runtime_state.record_cron_run(name, function_id, run_at);
                                },
                            }
                        }
                    }
                }
            },
            async {
                if introspection_enabled {
                    run_introspection_server(metrics.clone(), WorkersController::new(workers_tx), SendWrapper::new(runtime_state.clone())).await;
                }
            },
        )
    }));
}
