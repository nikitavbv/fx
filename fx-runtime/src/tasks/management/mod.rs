use {
    std::{collections::HashMap, sync::Arc},
    send_wrapper::SendWrapper,
    tokio::sync::oneshot,
    tracing::info,
    crate::{
        function::FunctionId,
        definitions::{config::{FunctionConfig, ServerConfig}, monitor::{DefinitionsMonitor, ApplyConfigError}},
        effects::metrics::{FunctionMetricsDelta, MetricsRegistry, MetricKey},
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
    DeployFunction(Box<DeployFunctionMessage>),
    WorkerMetrics(MetricsFlushMessage),
    FunctionInvoked(()),
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

    let (introspection_enabled, introspection_port) = config.introspection
        .map(|v| (v.enabled, v.port))
        .unwrap_or((true, 9000));

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
                                    let result = definitions_monitor.apply_config(msg.function_id, msg.function_config).await;
                                    msg.on_ready.send(()).unwrap();
                                    match result {
                                        Ok(_) => {},
                                        Err(ApplyConfigError::CronTaskShutdown) => {
                                            info!("stopping management task, because cron task is stopped");
                                            break;
                                        },
                                    }
                                },
                                ManagementMessage::WorkerMetrics(msg) => {
                                    metrics.update(msg.function_metrics);
                                },
                                ManagementMessage::FunctionInvoked(_msg) => {
                                    metrics.counter_increment(MetricKey::new("function_invoked"), 1);
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
                                CronTaskEvent::Start { name, function_id } => {
                                    runtime_state.mark_cron_running(name, function_id);
                                },
                                CronTaskEvent::Run { name, function_id, run_at, delay, iteration_delay } => {
                                    runtime_state.record_cron_run(name.clone(), function_id.clone(), run_at);
                                    let task_tag_value = format!("{}::{name}", function_id.as_str());
                                    if let Some(delay) = delay {
                                        metrics.counter_float_increment(MetricKey::new("cron_task_delay_seconds").with_label("task", &task_tag_value), delay.as_seconds_f64());
                                    }
                                    metrics.counter_float_increment(MetricKey::new("cron_task_iteration_delay_seconds").with_label("task", task_tag_value), iteration_delay.as_secs_f64());
                                },
                            }
                        }
                    }
                }
            },
            async {
                if introspection_enabled {
                    run_introspection_server(metrics.clone(), WorkersController::new(workers_tx), SendWrapper::new(runtime_state.clone()), introspection_port).await;
                }
            },
        )
    }));
}
