use {
    std::thread::JoinHandle,
    tracing::{error, info},
    tokio::sync::oneshot,
    crate::{
        definitions::config::{ServerConfig, LoggerConfig, FunctionConfig},
        effects::{
            logs::{LogMessageEvent, create_logger, Logger},
            sql::SqlDatabase,
        },
        tasks::{
            worker::{WorkerMessage, WorkerConfig, run_worker_task},
            sql::{SqlMessage, run_sql_task},
            compiler::CompilerMessage,
            management::{ManagementMessage, run_management_task, DeployFunctionMessage},
            cron::run_cron_task,
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
