use {
    std::{path::{PathBuf, Path}, cell::RefCell, collections::HashMap},
    tracing::{info, warn, error},
    tokio::{fs, sync::oneshot},
    walkdir::WalkDir,
    notify::Watcher,
    thiserror::Error,
    crate::{
        v2::{WorkerMessage, CompilerMessage, FunctionId, FunctionDeploymentId, FunctionHttpListener, SqlBindingConfig, SqlBindingConfigLocation, BlobBindingConfig},
        server::config::{ServerConfig, FunctionConfig, FunctionCodeConfig},
    },
};

const FILE_EXTENSION_WASM: &str = ".wasm";
const DEFINITION_FILE_SUFFIX: &str = ".fx.yaml";

pub(crate) struct DefinitionsMonitor {
    functions_directory: PathBuf,
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
    compiler_tx: flume::Sender<CompilerMessage>,

    // DefinitionsMonitor requests FunctionDeployment creation and assigns IDs to them.
    deployment_id_counter: RefCell<u64>,
}

impl DefinitionsMonitor {
    pub(crate) fn new(
        config: &ServerConfig,
        workers_tx: Vec<flume::Sender<WorkerMessage>>,
        compiler_tx: flume::Sender<CompilerMessage>,
    ) -> Self {
        Self {
            functions_directory: config.config_path.as_ref().unwrap().parent().unwrap()
                .join(&config.functions_dir),
            workers_tx,
            compiler_tx,
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
        let (tx, mut rx) = flume::unbounded();
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
            worker.send(WorkerMessage::RemoveFunction(function_id.clone())).unwrap();
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

        let bindings_sql = config.bindings.iter()
            .flat_map(|v| v.sql.iter())
            .flat_map(|v| v.iter())
            .map(|v| (v.id.clone(), SqlBindingConfig {
                connection_id: uuid::Uuid::new_v4().to_string(),
                location: match v.path.as_str() {
                    ":memory:" => SqlBindingConfigLocation::InMemory(uuid::Uuid::new_v4().to_string()),
                    path => SqlBindingConfigLocation::Path(config.config_path.as_ref().unwrap().parent().unwrap().join(&path)),
                },
            }))
            .collect::<HashMap<_, _>>();

        let bindings_blob = config.bindings.iter()
            .flat_map(|v| v.blob.iter())
            .flat_map(|v| v.iter())
            .map(|v| (v.id.clone(), BlobBindingConfig {
                storage_directory: config.config_path.as_ref().unwrap().parent().unwrap().join(&v.path),
            }))
            .collect::<HashMap<_, _>>();

        for worker in self.workers_tx.iter() {
            worker.send_async(WorkerMessage::FunctionDeploy {
                function_id: function_id.clone(),
                deployment_id: deployment_id.clone(),
                module: module.clone(),
                http_listeners: http_listeners.clone(),
                bindings_sql: bindings_sql.clone(),
                bindings_blob: bindings_blob.clone(),
            }).await.unwrap();
        }
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
