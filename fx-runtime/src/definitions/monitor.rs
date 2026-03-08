use {
    std::{
        cell::RefCell,
        collections::HashMap,
        path::{Path, PathBuf},
        time::Duration,
        str::FromStr,
    },
    notify::Watcher,
    thiserror::Error,
    tokio::{fs, sync::oneshot},
    tracing::{error, info, warn},
    walkdir::WalkDir,
    crate::{
        tasks::{
            worker::WorkerMessage,
            compiler::CompilerMessage,
            cron::CronMessage,
        },
        definitions::{
            config::{ServerConfig, FunctionConfig, FunctionCodeConfig, EnvValueSource},
            triggers::{FunctionHttpListener, CronTrigger},
            bindings::{SqlBindingConfig, SqlBindingConfigLocation, BlobBindingConfig, KvBindingConfig, FunctionBindingConfig},
        },
        function::{FunctionId, FunctionDeploymentId},
    },
};

const DEFINITION_FILE_SUFFIX: &str = ".fx.yaml";

pub(crate) struct DefinitionsMonitor {
    functions_directory: PathBuf,
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
    compiler_tx: flume::Sender<CompilerMessage>,
    cron_tx: flume::Sender<CronMessage>,

    // DefinitionsMonitor requests FunctionDeployment creation and assigns IDs to them.
    deployment_id_counter: RefCell<u64>,
}

impl DefinitionsMonitor {
    pub(crate) fn new(
        config: &ServerConfig,
        workers_tx: Vec<flume::Sender<WorkerMessage>>,
        compiler_tx: flume::Sender<CompilerMessage>,
        cron_tx: flume::Sender<CronMessage>,
    ) -> Self {
        Self {
            functions_directory: config.config_path.as_ref().unwrap().parent().unwrap()
                .join(&config.functions_dir),
            workers_tx,
            compiler_tx,
            cron_tx,
            deployment_id_counter: RefCell::new(0),
        }
    }

    pub(crate) async fn scan_definitions(&self) {
        info!("will scan definitions in: {:?}", self.functions_directory);

        let root = self.functions_directory.clone();
        for entry in WalkDir::new(&root) {
            let entry = match entry {
                Ok(v) => v,
                Err(err) => {
                    warn!("failed to scan definitions in dir: {err:?}");
                    continue;
                }
            };

            if !entry.file_type().is_file() {
                continue;
            }

            let entry_path = entry.path();
            let function_id = match self.path_to_function_id(entry_path) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let function_config = match FunctionConfig::load(entry_path.to_path_buf()).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to load function config: {err:?}");
                    continue;
                }
            };

            self.apply_config(function_id, function_config).await;
        }

        info!("listening for definition changes");
        let (tx, rx) = flume::unbounded();
        let event_fn = {
            move |res: notify::Result<notify::Event>| {
                let res = res.unwrap();

                match res.kind {
                    notify::EventKind::Access(_) => {},
                    _other => {
                        for changed_path in res.paths {
                            tx.send(changed_path).unwrap();
                        }
                    }
                }
            }
        };
        let mut watcher = notify::recommended_watcher(event_fn).unwrap();
        if let Err(err) = watcher.watch(&root, notify::RecursiveMode::Recursive) {
            error!("failed to start watching functions directory: {err:?}");
            return;
        }

        while let Ok(path) = rx.recv_async().await {
            let function_id = match self.path_to_function_id(&path) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !fs::try_exists(&path).await.unwrap() {
                self.remove_function(function_id);
                continue;
            }

            let metadata = fs::metadata(&path).await.unwrap();
            if !metadata.is_file() {
                continue;
            }

            let function_config = match FunctionConfig::load(path).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to load function config: {err:?}");
                    continue
                }
            };

            self.apply_config(function_id, function_config).await;
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

    fn remove_function(&self, function_id: FunctionId) {
        info!("removing function: {:?}", function_id.as_string());
        for worker in self.workers_tx.iter() {
            worker.send(WorkerMessage::RemoveFunction { function_id: function_id.clone(), on_ready: None }).unwrap();
        }
    }

    pub(crate) async fn apply_config(&self, function_id: FunctionId, config: FunctionConfig) {
        info!("applying config for: {:?}", function_id.as_string());

        // first, precompile module:
        let module_code = match config.code {
            Some(v) => match v {
                FunctionCodeConfig::Path(v) => fs::read(&v).await.unwrap(),
                FunctionCodeConfig::Inline(v) => v,
            },
            None => {
                let module_code = self.function_id_to_path(&function_id).with_added_extension("wasm");
                fs::read(&module_code).await.unwrap()
            }
        };

        let (compiler_response_tx, compiler_response_rx) = oneshot::channel();
        self.compiler_tx.send_async(CompilerMessage {
            function_id: function_id.clone(),
            code: module_code,
            response: compiler_response_tx,
        }).await.unwrap();

        let deployment_id = self.next_deployment_id();
        let module = compiler_response_rx.await.unwrap();

        let http_listeners = config.triggers.iter()
            .flat_map(|v| v.http.iter())
            .flat_map(|v| v.iter())
            .map(|v| FunctionHttpListener {
                host: v.host.clone(),
            })
            .collect::<Vec<_>>();

        let env = {
            let mut env = HashMap::new();
            for var in config.env.into_iter().flatten() {
                env.insert(var.id, match var.source {
                    EnvValueSource::Value(v) => v,
                    EnvValueSource::File(v) => fs::read_to_string(v).await.unwrap(),
                });
            }

            env
        };

        let bindings_sql = config.bindings.iter()
            .flat_map(|v| v.sql.iter())
            .flat_map(|v| v.iter())
            .map(|v| (v.id.clone(), SqlBindingConfig {
                connection_id: uuid::Uuid::new_v4().to_string(),
                location: match v.path.as_str() {
                    ":memory:" => SqlBindingConfigLocation::InMemory(uuid::Uuid::new_v4().to_string()),
                    path => SqlBindingConfigLocation::Path(config.config_path.as_ref().unwrap().parent().unwrap().join(&path)),
                },
                busy_timeout: v.busy_timeout_ms.map(|v| Duration::from_millis(v)),
            }))
            .collect::<HashMap<_, _>>();

        let bindings_blob = config.bindings.iter()
            .flat_map(|v| v.blob.iter())
            .flat_map(|v| v.iter())
            .map(|v| (v.id.clone(), BlobBindingConfig {
                bucket: v.bucket.clone(),
            }))
            .collect::<HashMap<_, _>>();

        let bindings_kv = config.bindings.iter()
            .flat_map(|v| v.kv.iter())
            .flat_map(|v| v.iter())
            .map(|v| (v.id.clone(), KvBindingConfig {
                namespace: v.namespace.clone(),
            }))
            .collect::<HashMap<_, _>>();

        let bindings_functions = config.bindings.iter()
            .flat_map(|v| v.functions.iter())
            .flat_map(|v| v.iter())
            .map(|v| (
                v.host.clone().unwrap_or(v.function_id.clone()).to_lowercase(),
                FunctionBindingConfig {
                    function_id: FunctionId::new(&v.function_id),
                }
            ))
            .collect::<HashMap<_, _>>();

        for worker in self.workers_tx.iter() {
            worker.send_async(WorkerMessage::FunctionDeploy {
                function_id: function_id.clone(),
                deployment_id: deployment_id.clone(),
                module: module.clone(),
                http_listeners: http_listeners.clone(),
                env: env.clone(),
                bindings_sql: bindings_sql.clone(),
                bindings_blob: bindings_blob.clone(),
                bindings_kv: bindings_kv.clone(),
                bindings_functions: bindings_functions.clone(),
            }).await.unwrap();
        }

        let cron_triggers = config.triggers.iter()
            .flat_map(|v| v.cron.iter())
            .flat_map(|v| v.iter())
            .map(|v| CronTrigger {
                id: format!("{}::{}", function_id.as_str(), v.id),
                schedule: cron::Schedule::from_str(&v.schedule).unwrap(),
            })
            .collect::<Vec<_>>();

        self.cron_tx.send_async(CronMessage::ScheduleAdd {
            function_id,
            schedule: cron_triggers,
        }).await.unwrap();
    }

    fn next_deployment_id(&self) -> FunctionDeploymentId {
        let deployment_id = FunctionDeploymentId::new(*self.deployment_id_counter.borrow());
        *self.deployment_id_counter.borrow_mut() += 1;
        deployment_id
    }
}

#[derive(Error, Debug)]
enum FunctionIdDetectionError {
    #[error("config path missing .fx.yaml extension")]
    PathMissingExtension,
}
