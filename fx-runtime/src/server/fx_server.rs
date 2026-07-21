use {
    std::thread::JoinHandle,
    tracing::{error, info},
    tokio::sync::oneshot,
    thiserror::Error,
    crate::{
        definitions::config::{ServerConfig, LoggerConfig, FunctionConfig},
        effects::{
            logs::{LogMessageEvent, create_logger, Logger},
            sql::SqlDatabase,
        },
        tasks::{
            worker::{WorkerMessage, WorkerConfig, run_worker_task, WorkersController},
            sql::{SqlMessage, run_sql_task, SqlController},
            compiler::{CompilerMessage, CompilerError},
            management::{ManagementMessage, run_management_task, DeployFunctionMessage},
            cron::{run_cron_task, CronTaskEvent},
            kv::run_kv_task,
            blob::run_blob_task,
        },
        triggers::cron::CronDatabase,
        function::FunctionId,
    },
};

pub struct FxServer {
    config: ServerConfig,
}

impl FxServer {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }
}

mod start {
    use super::*;

    #[derive(Debug, Error)]
    pub enum StartError {
        #[error("failed to init runtime because of internal error")]
        InternalInitError,
    }

    impl FxServer {
        pub fn start(self) -> Result<RunningFxServer, StartError> {
            let wasmtime = wasmtime::Engine::new(
                wasmtime::Config::new()
                    .async_support(true)
                    .epoch_interruption(true)
            ).map_err(|err| {
                error!("failed to create wasmtime engine: {err:?}");
                StartError::InternalInitError
            })?;

            let cpu_info = match gdt_cpus::CpuInfo::detect() {
                Ok(v) => Some(v),
                Err(err) => {
                    error!("failed to get cpu info: {err:?}");
                    None
                }
            };

            let target_workers = self.config.workers.unwrap_or(4);

            let worker_threads = target_workers.min(cpu_info.as_ref().map(|v| v.num_logical_cores()).unwrap_or(usize::MAX));
            let sql_threads = target_workers.min(cpu_info.as_ref().map(|v| v.num_logical_cores()).unwrap_or(usize::MAX));

            let (any_worker_tx, any_worker_rx) = flume::unbounded::<WorkerMessage>();
            let (workers_tx, workers_rx) = (0..worker_threads)
                .map(|_| flume::unbounded::<WorkerMessage>())
                .unzip::<_, _, Vec<_>, Vec<_>>();
            let workers_controller = WorkersController::new(any_worker_tx.clone(), workers_tx.clone());

            let (sql_tx, sql_rx) = flume::unbounded::<SqlMessage>();
            let (sql_thread_tx, sql_thread_rx) = (0..sql_threads)
                .map(|_| flume::unbounded::<SqlMessage>())
                .unzip::<_, _, Vec<_>, Vec<_>>();
            let sql_controller = SqlController::new(sql_tx, sql_thread_tx);

            let (compiler_tx, compiler_rx) = flume::unbounded::<CompilerMessage>();
            let (management_tx, management_rx) = flume::unbounded::<ManagementMessage>();
            let (logger_tx, logger_rx) = flume::unbounded::<LogMessageEvent>();
            let (cron_tx, cron_rx) = flume::unbounded();
            let (cron_event_tx, cron_event_rx) = flume::unbounded::<CronTaskEvent>();
            let (kv_tx, kv_rx) = flume::unbounded();
            let (blob_tx, blob_rx) = flume::unbounded();

            let timer_thread_handle = {
                let wasmtime = wasmtime.weak();

                std::thread::spawn(move || {
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        let wasmtime = match wasmtime.upgrade() {
                            Some(v) => v,
                            None => break,
                        };

                        wasmtime.increment_epoch();
                    }
                })
            };

            let management_thread_handle = {
                let config = self.config.clone();
                let workers_tx = workers_tx.clone();

                std::thread::spawn(move || {
                    info!("started management thread");

                    run_management_task(config, any_worker_tx, workers_tx, compiler_tx, cron_tx, cron_event_rx, management_rx);
                })
            };

            let compiler_thread_handle = {
                let wasmtime = wasmtime.clone();

                std::thread::spawn(move || {
                    info!("started compiler thread");

                    while let Ok(msg) = compiler_rx.recv() {
                        let function_id = msg.function_id.as_string();

                        info!(function_id, "compiling");
                        let _ = msg.response.send(
                            wasmtime::Module::new(&wasmtime, msg.code)
                                .map_err(|err| {
                                    error!("failed to compile wasm module: {err:?}");
                                    CompilerError::FailedToCompile
                                })
                        );
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
                std::thread::spawn(move || {
                    let cron_database = match self.config.cron_data_path {
                        Some(cron_data_path) => {
                            let config_path = match self.config.config_path {
                                Some(v) => v,
                                None => {
                                    error!("skipping cron thread from starting: expected config path to be set.");
                                    return;
                                }
                            };

                            let fx_root_dir = match config_path.parent() {
                                Some(v) => v,
                                None => {
                                    error!("skipping cron thread from starting: failed to get fx root dir based on config path.");
                                    return;
                                }
                            };

                            match SqlDatabase::new(fx_root_dir.join(cron_data_path)) {
                                Ok(v) => v,
                                Err(err) => {
                                    error!("skipping cron thread from starting: failed to create sql database: {err:?}");
                                    return;
                                },
                            }
                        },
                        None => match SqlDatabase::in_memory() {
                            Ok(v) => v,
                            Err(err) => {
                                error!("skipping cron thread from starting: failed to create in-memory sql database: {err:?}");
                                return;
                            },
                        },
                    };

                    let cron_database = CronDatabase::new(cron_database);

                    let cron_database = match cron_database {
                        Ok(v) => v,
                        Err(_) => {
                            error!("skipping cron thread from starting: failed to create cron database.");
                            return;
                        }
                    };

                    info!("started cron thread");
                    run_cron_task(cron_database, workers_controller, cron_rx, cron_event_tx);
                })
            };

            let sql_cores = cpu_info.as_ref()
                .map(|v| v.logical_processor_ids().iter().take(sql_threads).map(|v| Some(*v)).collect::<Vec<_>>())
                .unwrap_or(std::iter::repeat_n(None, sql_threads).collect());

            let mut sql_worker_handles = Vec::new();
            for (sql_worker_id, (sql_worker, sql_thread_rx)) in sql_cores.into_iter().zip(sql_thread_rx).enumerate() {
                let sql_rx = sql_rx.clone();
                let config = self.config.sql.as_ref().cloned().unwrap_or_default();

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

                    run_sql_task(config.path, sql_rx, sql_thread_rx);
                });
                sql_worker_handles.push(handle);
            }

            let kv_thread_handle = {
                std::thread::spawn(move || {
                    info!("started kv thread");
                    run_kv_task(kv_rx);
                })
            };

            let blob_thread_handle = {
                let config = self.config.blob.as_ref().cloned().unwrap_or_default();

                std::thread::spawn(move || {
                    info!("started blob thread");
                    run_blob_task(config.path, blob_rx);
                })
            };

            let worker_cores = cpu_info.as_ref()
                .map(|v| v.logical_processor_ids().iter().take(worker_threads).map(|v| Some(*v)).collect::<Vec<_>>())
                .unwrap_or(std::iter::repeat_n(None, worker_threads).collect());

            let workers = worker_cores.into_iter()
                .zip(workers_rx)
                .map(|(core_id, messages_rx)| WorkerConfig {
                    core_id,
                    port: self.config.server.port.value,
                    messages_rx,
                    messages_shared_rx: any_worker_rx.clone(),
                    sql_controller: sql_controller.clone(),
                    kv_tx: kv_tx.clone(),
                    blob_tx: blob_tx.clone(),
                    logger_tx: logger_tx.clone(),
                    management_tx: management_tx.clone(),
                })
                .collect::<Vec<_>>();

            let mut worker_handles = Vec::new();
            for (worker_id, worker) in workers.into_iter().enumerate() {
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
            }

            Ok(RunningFxServer {
                management_tx,

                worker_handles,
                sql_worker_handles,
                compiler_thread_handle,
                management_thread_handle,
                logger_thread_handle,
                cron_thread_handle,
                kv_thread_handle,
                blob_thread_handle,
                timer_thread_handle,
            })
        }
    }
}

pub struct RunningFxServer {
    management_tx: flume::Sender<ManagementMessage>,

    worker_handles: Vec<JoinHandle<()>>,
    sql_worker_handles: Vec<JoinHandle<()>>,
    compiler_thread_handle: JoinHandle<()>,
    management_thread_handle: JoinHandle<()>,
    logger_thread_handle: JoinHandle<()>,
    cron_thread_handle: JoinHandle<()>,
    kv_thread_handle: JoinHandle<()>,
    blob_thread_handle: JoinHandle<()>,
    timer_thread_handle: JoinHandle<()>,
}

impl RunningFxServer {
    pub fn wait_until_finished(self) {
        for handle in self.worker_handles {
            if let Err(err) = handle.join() {
                error!("worker thread panic detected: {err:?}");
            }
        }
        for handle in self.sql_worker_handles {
            if let Err(err) = handle.join() {
                error!("sql worker thread panic detected: {err:?}");
            }
        }
        wait_until_finished("kv", self.kv_thread_handle);
        wait_until_finished("blob", self.blob_thread_handle);
        wait_until_finished("cron", self.cron_thread_handle);
        wait_until_finished("compiler", self.compiler_thread_handle);
        wait_until_finished("logger", self.logger_thread_handle);
        wait_until_finished("management", self.management_thread_handle);
        wait_until_finished("timer", self.timer_thread_handle);
        info!("server shutdown all threads.");
    }
}

mod deploy_function {
    use super::*;

    #[derive(Debug, Error)]
    pub enum DeployError {
        #[error("failed to deploy function because runtime is being shut down")]
        RuntimeShutdown,
        #[error("failed to deploy function because failed to compile wasm module")]
        CompileError,
    }

    impl From<crate::tasks::management::DeployFunctionError> for DeployError {
        fn from(err: crate::tasks::management::DeployFunctionError) -> Self {
            use crate::tasks::management::DeployFunctionError as SourceError;
            match err {
                SourceError::CompileError => Self::CompileError,
            }
        }
    }

    impl RunningFxServer {
        #[allow(dead_code)]
        pub async fn deploy_function(&self, function_id: FunctionId, function_config: FunctionConfig) -> Result<(), DeployError> {
            let (response_tx, response_rx) = oneshot::channel();

            self.management_tx.send_async(ManagementMessage::DeployFunction(Box::new(DeployFunctionMessage { function_id, function_config, on_ready: response_tx }))).await
                .map_err(|_| DeployError::RuntimeShutdown)?;

            response_rx.await.map_err(|_| DeployError::RuntimeShutdown)?.map_err(DeployError::from)
        }
    }
}

fn wait_until_finished(thread_name: &'static str, join_handle: JoinHandle<()>) {
    if let Err(err) = join_handle.join() {
        error!("thread panic detected, thread = {thread_name}, error: {err:?}");
    }
}
