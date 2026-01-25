use {
    std::{sync::Arc, path::{PathBuf, Path}, collections::HashMap, net::SocketAddr, str::FromStr},
    tokio::{
        fs,
        sync::{Mutex, RwLock, mpsc},
        net::TcpListener,
        time::{sleep, Duration},
    },
    hyper::server::conn::http1,
    hyper_util::rt::{TokioIo, TokioTimer},
    tracing::{info, warn, error},
    walkdir::WalkDir,
    notify::Watcher,
    thiserror::Error,
    cron as cron_utils,
    chrono::Utc,
    crate::{
        runtime::{
            FxRuntime,
            FunctionId,
            sql::SqlDatabase,
            runtime::{RpcBinding, FunctionInvokeAndExecuteError, FunctionInvocationEvent},
            logs::{StdoutLogger, NoopLogger, BoxLogger},
        },
        server::{
            config::{ServerConfig, FunctionConfig, FunctionCodeConfig, LoggerConfig},
            logs::RabbitMqLogger,
            http::HttpHandler,
            cron::CronDatabase,
        },
    },
};

const DEFINITION_FILE_SUFFIX: &str = ".fx.yaml";

pub struct FxServer {
    runtime: Arc<FxRuntime>,
    definitions_monitor: DefinitionsMonitor,

    config: ServerConfig,

    http_definition_rx: Arc<Mutex<mpsc::Receiver<HttpListenerDefinition>>>,
    cron_definition_rx: Arc<Mutex<mpsc::Receiver<CronListenerDefinition>>>,
}

struct DefinitionsMonitor {
    runtime: Arc<FxRuntime>,
    functions_directory: PathBuf,

    http_definition_tx: Arc<Mutex<mpsc::Sender<HttpListenerDefinition>>>,
    cron_definition_tx: Arc<Mutex<mpsc::Sender<CronListenerDefinition>>>,

    listeners_state: Arc<Mutex<ListenersState>>,
}

struct ListenersState {
    http: HttpListenerDefinition,
    cron: CronListenerDefinition,
}

#[derive(Clone, Debug)]
struct HttpListenerDefinition {
    function: Option<FunctionId>,
}

#[derive(Clone, Debug)]
struct CronListenerDefinition {
    schedule: Vec<CronListenerEntry>,
}

#[derive(Clone, Debug)]
struct CronListenerEntry {
    task_id: String,
    function_id: FunctionId,
    handler: String,
    schedule: cron_utils::Schedule,
}

impl FxServer {
    pub async fn new(config: ServerConfig, mut runtime: FxRuntime) -> Self {
        if let Some(logger) = config.logger.as_ref() {
            runtime = runtime.with_logger(match logger {
                LoggerConfig::Stdout => BoxLogger::new(StdoutLogger::new()),
                LoggerConfig::Noop => BoxLogger::new(NoopLogger::new()),
                LoggerConfig::RabbitMq { uri, exchange } => BoxLogger::new(RabbitMqLogger::new(uri.clone(), exchange.clone()).await.unwrap()),
            });
        }
        let runtime = Arc::new(runtime);

        // TODO: tokio::watch is better here?
        let (http_tx, http_rx) = mpsc::channel::<HttpListenerDefinition>(100);
        let (cron_tx, cron_rx) = mpsc::channel::<CronListenerDefinition>(100);

        let definitions_monitor = DefinitionsMonitor::new(
            runtime.clone(),
            &config,
            http_tx,
            cron_tx,
        );

        Self {
            runtime,
            definitions_monitor,

            config,

            http_definition_rx: Arc::new(Mutex::new(http_rx)),
            cron_definition_rx: Arc::new(Mutex::new(cron_rx)),
        }
    }

    pub async fn serve(&self) {
        info!("starting fx server");

        tokio::join!(
            self.definitions_monitor.scan_definitions(),
            self.run_http_listener(),
            self.run_cron_listener(),
        );
    }

    /// Note: cannot be used together with `serve`. Provided for testing and for
    /// building very custom servers.
    pub async fn define_function(&self, function_id: FunctionId, config: FunctionConfig) {
        self.definitions_monitor.define_function(function_id, config).await;
    }

    pub async fn invoke_function<T: serde::ser::Serialize, S: serde::de::DeserializeOwned>(&self, function_id: &FunctionId, handler_name: &str, arg: T) -> Result<(S, FunctionInvocationEvent), FunctionInvokeAndExecuteError> {
        self.runtime.engine.invoke_service(self.runtime.engine.clone(), &function_id, handler_name, arg).await
    }

    async fn run_http_listener(&self) {
        let mut http_definition_rx = self.http_definition_rx.lock().await;
        while let Some(mut definition) = http_definition_rx.recv().await {
            let server_function = match definition.function {
                Some(v) => v,
                None => {
                    // target function not defined, continue sleeping...
                    continue;
                }
            };

            // TODO: take port from config
            let addr: SocketAddr = ([0, 0, 0, 0], 8080).into();
            let listener = match TcpListener::bind(addr).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to bind tcp listener for http sever: {err:?}");
                    continue;
                }
            };
            let graceful = hyper_util::server::graceful::GracefulShutdown::new();

            let http_handler = Arc::new(HttpHandler::new(self.runtime.clone(), server_function.clone()));

            info!("started http server on {addr:?}");
            loop {
                tokio::select! {
                    new_definition = http_definition_rx.recv() => {
                        let new_definition = match new_definition {
                            Some(v) => v,
                            None => {
                                continue
                            }
                        };

                        if let Some(new_target_function) = new_definition.function {
                            http_handler.update_target_function(new_target_function);
                        } else {
                            info!("no http listener is set - stopping http server.");
                            drop(listener);
                            break;
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

                        let http_handler = http_handler.clone();
                        let conn = http1::Builder::new()
                            .timer(TokioTimer::new())
                            .serve_connection(io, http_handler);
                        let fut = graceful.watch(conn);
                        tokio::task::spawn(async move {
                            if let Err(err) = fut.await {
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
            info!("stopped http server.");
        }
    }

    async fn run_cron_listener(&self) {
        let mut cron_definition_rx = self.cron_definition_rx.lock().await;
        while let Some(definition) = cron_definition_rx.recv().await {
            if definition.schedule.is_empty() {
                // empty schedule, nothing to do, wait until next configuration update...
                continue;
            }

            let mut tasks = definition.schedule;

            let cron_data_path = match self.config.cron_data_path.clone() {
                Some(v) => v,
                None => {
                    error!("using cron requires specifying cron_data_path in server config. Update config and restart server to use cron.");
                    break;
                }
            };

            let database = CronDatabase::new(SqlDatabase::new(&cron_data_path).unwrap());

            info!("started cron scheduler");

            loop {
                tokio::select! {
                    new_definition = cron_definition_rx.recv() => {
                        let new_definition = match new_definition {
                            Some(v) => v,
                            None => {
                                continue;
                            }
                        };

                        tasks = new_definition.schedule;

                        if tasks.is_empty() {
                            info!("no cron tasks are defined. Stopping cron scheduler.");
                            break;
                        }
                    },
                    _tick = sleep(Duration::from_secs(1)) => {
                        for task in &tasks {
                            let now = Utc::now();
                            match database.get_prev_run_time(&task.task_id.as_str()) {
                                None => {
                                    // first time, let's run
                                },
                                Some(v) => if task.schedule.after(&v).next().unwrap() <= now {
                                    // time to run
                                } else {
                                    // too early to run again
                                    continue;
                                },
                            };

                            let result = self.runtime.engine.invoke_service::<(), ()>(self.runtime.engine.clone(), &task.function_id, &task.handler, ()).await;
                            match result {
                                Ok(_) => {
                                    database.update_run_time(&task.task_id, now);
                                },
                                Err(err) => {
                                    error!("failed to run cron task: {err:?}. Will try again...");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl DefinitionsMonitor {
    pub fn new(
        runtime: Arc<FxRuntime>,
        config: &ServerConfig,
        http_definition_tx: mpsc::Sender<HttpListenerDefinition>,
        cron_definition_tx: mpsc::Sender<CronListenerDefinition>,
    ) -> Self {
        Self {
            runtime,
            functions_directory: config.config_path.as_ref().unwrap().parent().unwrap().join(&config.functions_dir),
            http_definition_tx: Arc::new(Mutex::new(http_definition_tx)),
            cron_definition_tx: Arc::new(Mutex::new(cron_definition_tx)),
            listeners_state: Arc::new(Mutex::new(ListenersState::new())),
        }
    }

    pub async fn scan_definitions(&self) {
        info!("will scan definitions in {:?}", self.functions_directory);

        let mut listeners_state = self.listeners_state.lock().await;

        let root = &self.functions_directory;
        for entry in WalkDir::new(root) {
            let entry = match entry {
                Ok(v) => v,
                Err(err) => {
                    warn!("failed to scan definitions in dir: {err:?}");
                    continue;
                },
            };

            if !entry.file_type().is_file() {
                continue;
            }

            let entry_path = entry.path();
            let function_id = match self.path_to_function_id(entry_path) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if !fs::try_exists(&entry_path).await.unwrap() {
                warn!("config for function {function_id:?} does not exist");
                continue;
            }
            let function_config = match FunctionConfig::load(entry_path.to_path_buf()).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to load function config: {err:?}");
                    continue;
                }
            };

            self.apply_config(
                function_id,
                function_config,
                &mut listeners_state,
            ).await;
        }

        info!("listening for definition changes...");

        let (tx, mut rx) = mpsc::channel(1024);
        let event_fn = {
            move |res: notify::Result<notify::Event>| {
                let res = res.unwrap();

                match res.kind {
                    notify::EventKind::Access(_) => {},
                    _other => {
                        for changed_path in res.paths {
                            tx.blocking_send(changed_path).unwrap();
                        }
                    }
                }
            }
        };
        let mut watcher = notify::recommended_watcher(event_fn).unwrap();
        watcher.watch(&root, notify::RecursiveMode::Recursive).unwrap();

        while let Some(path) = rx.recv().await {
            let function_id = match self.path_to_function_id(&path) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !fs::try_exists(&path).await.unwrap() {
                self.remove_function(function_id, &mut listeners_state.http).await;
                continue;
            }

            let metadata = tokio::fs::metadata(&path).await.unwrap();
            if !metadata.is_file() {
                continue;
            }

            let function_config = match FunctionConfig::load(path.to_path_buf()).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to load function config: {err:?}");
                    continue;
                }
            };

            self.apply_config(
                function_id,
                function_config,
                &mut listeners_state,
            ).await;
        }
    }

    /// Note: cannot be used together with `scan_definitions`. Provided for testing and for
    /// building very custom servers.
    pub async fn define_function(&self, function_id: FunctionId, config: FunctionConfig) {
        let mut listeners_state = self.listeners_state.lock().await;
        self.apply_config(function_id, config, &mut listeners_state).await;
    }

    async fn apply_config(
        &self,
        function_id: FunctionId,
        config: FunctionConfig,
        listeners_state: &mut ListenersState,
    ) {
        info!("applying config for {:?}", function_id.as_string());

        // first, precompile module:
        let module_code = match config.code {
            Some(v) => match v {
                FunctionCodeConfig::Path(v) => fs::read(&v).await.unwrap(),
                FunctionCodeConfig::Inline(v) => v,
            },
            None => {
                let module_code = self.function_id_to_path(&function_id).with_added_extension("wasm");
                fs::read(&module_code).await.unwrap()
            },
        };

        let runtime = self.runtime.clone();
        let compiled_module = {
            let function_id = function_id.clone();

            tokio::task::spawn_blocking(move || {
                info!("compiling {function_id:?}");
                let module = runtime.engine.compile_module(&module_code);
                info!("finished compiling {function_id:?}");
                module
            }).await.unwrap()
        };

        // TODO: bindings should be lazy
        let mut sql = HashMap::new();
        for binding in config.bindings.as_ref().and_then(|v| v.sql.as_ref()).unwrap_or(&Vec::new()) {
            let path = config.config_path.as_ref().unwrap().parent().unwrap().join(&binding.path);
            sql.insert(binding.id.clone(), SqlDatabase::new(path).unwrap());
        }

        let mut rpc = HashMap::new();
        for binding in config.bindings.as_ref().and_then(|v| v.rpc.as_ref()).unwrap_or(&Vec::new()) {
            let function_path = config.config_path.as_ref().unwrap().parent().unwrap().join(&binding.function);
            let function_id = match self.path_to_function_id(&function_path) {
                Ok(v) => v,
                Err(err) => {
                    warn!("failed to detect function id for rpc binding: {err:?}");
                    continue;
                }
            };
            rpc.insert(binding.id.clone(), RpcBinding::new(function_id));
        }

        let execution_context = match self.runtime.engine.create_execution_context_v2(self.runtime.engine.clone(), function_id.clone(), compiled_module, sql, rpc) {
            Ok(v) => v,
            Err(err) => {
                error!("failed to create execution context for function: {err:?}");
                return;
            }
        };
        let prev_execution_context = self.runtime.engine.update_function_execution_context(function_id.clone(), execution_context);
        if let Some(prev_execution_context) = prev_execution_context {
            // TODO: graceful drain - cleanup in background job?
            self.runtime.engine.remove_execution_context(&prev_execution_context);
        }

        // second, configure triggers:
        // http:
        let http_trigger_enabled = config.triggers.as_ref()
            .map(|triggers| !triggers.http.as_ref().map(|v| v.is_empty()).unwrap_or(true))
            .unwrap_or(false);

        if http_trigger_enabled {
            if let Some(existing_handler) = listeners_state.http.function.as_ref() {
                if existing_handler != &function_id {
                    panic!("http listener already set to a different function: {:?}", existing_handler.as_string());
                } else {
                    // same listener as current, nothing to do
                }
            } else {
                // no http listener configured, let's set one
                listeners_state.http.function = Some(function_id.clone());
                self.http_definition_tx.lock().await.send(listeners_state.http.clone()).await.unwrap();
            }
        } else {
            if let Some(existing_handler) = listeners_state.http.function.as_ref() {
                if existing_handler != &function_id {
                    // existing handler set to a different function, nothing to do
                } else {
                    listeners_state.http.function = None;
                    self.http_definition_tx.lock().await.send(listeners_state.http.clone()).await.unwrap();
                }
            } else {
                // no http listener configured, nothing to do
            }
        }

        // cron:
        let config_cron = config.triggers.as_ref().and_then(|v| v.cron.as_ref()).cloned().unwrap_or(Vec::new());
        if !config_cron.is_empty() {
            listeners_state.cron.schedule = listeners_state.cron.schedule
                .iter()
                .cloned()
                .filter(|v| &v.function_id != &function_id)
                .chain(config_cron.into_iter().map(|v| CronListenerEntry {
                    task_id: format!("{}/{}", function_id.as_string(), v.id),
                    function_id: function_id.clone(),
                    handler: v.handler.clone(),
                    schedule: cron_utils::Schedule::from_str(&v.schedule).unwrap(),
                }))
                .collect();

            self.cron_definition_tx.lock().await.send(listeners_state.cron.clone()).await.unwrap();
        }
    }

    async fn remove_function(&self, function_id: FunctionId, definition_http: &mut HttpListenerDefinition) {
        info!("removing function: {function_id:?}");

        let execution_context = self.runtime.engine.resolve_context_id_for_function(&function_id);
        // TODO: graceful drain - cleanup in background job?
        self.runtime.engine.remove_execution_context(&execution_context);

        if definition_http.function.as_ref().map(|v| v == &function_id).unwrap_or(false) {
            definition_http.function = None;
            self.http_definition_tx.lock().await.send(definition_http.clone()).await.unwrap();
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
}

impl ListenersState {
    pub fn new() -> Self {
        Self {
            http: HttpListenerDefinition {
                function: None,
            },
            cron: CronListenerDefinition {
                schedule: Vec::new(),
            },
        }
    }
}

#[derive(Error, Debug)]
enum FunctionIdDetectionError {
    #[error("config path missing .fx.yaml extension")]
    PathMissingExtension,
}
