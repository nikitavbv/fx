use {
    std::{sync::Arc, path::{PathBuf, Path}, collections::HashMap, net::SocketAddr},
    tokio::{
        fs,
        sync::{Mutex, RwLock, mpsc},
        net::TcpListener,
    },
    tracing::{info, warn, error},
    walkdir::WalkDir,
    notify::Watcher,
    crate::{
        runtime::{FxRuntime, FunctionId},
        server::config::{ServerConfig, FunctionConfig},
    },
};

const DEFINITION_FILE_SUFFIX: &str = ".fx.yaml";
const EMPTY_FUNCTION_CONFIG: FunctionConfig = FunctionConfig {
    triggers: None,
};

pub struct FxServer {
    runtime: Arc<FxRuntime>,
    definitions_monitor: DefinitionsMonitor,

    http_definition_rx: Arc<Mutex<mpsc::Receiver<HttpListenerDefinition>>>,
}

struct DefinitionsMonitor {
    runtime: Arc<FxRuntime>,
    functions_directory: PathBuf,
    functions_configs: Arc<Mutex<HashMap<FunctionId, FunctionConfig>>>,

    http_definition_tx: Arc<Mutex<mpsc::Sender<HttpListenerDefinition>>>,
}

#[derive(Clone)]
struct HttpListenerDefinition {
    function: Option<FunctionId>,
}

struct DefinitionSubscribers {
    http: mpsc::Receiver<HttpListenerDefinition>,
}

impl FxServer {
    pub fn new(config: ServerConfig, runtime: FxRuntime) -> Self {
        let runtime = Arc::new(runtime);

        let (http_tx, http_rx) = mpsc::channel::<HttpListenerDefinition>(100);

        let definitions_monitor = DefinitionsMonitor::new(
            runtime.clone(),
            &config,
            http_tx,
        );

        Self {
            runtime,
            definitions_monitor,

            http_definition_rx: Arc::new(Mutex::new(http_rx)),
        }
    }

    pub async fn serve(&self) {
        tokio::join!(
            self.definitions_monitor.scan_definitions(),
            self.run_http_listener()
        );
    }

    async fn run_http_listener(&self) {
        let mut http_definition_tx = self.http_definition_rx.lock().await;
        while let Some(mut definition) = http_definition_tx.recv().await {
            let server_function = match definition.function {
                Some(v) => v,
                None => {
                    // target function not defined, continue sleeping...
                    continue;
                }
            };

            let addr: SocketAddr = ([0, 0, 0, 0], 8080).into();
            let listener = match TcpListener::bind(addr).await {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to bind tcp listener for http sever: {err:?}");
                    continue;
                }
            };

            info!("started http server on {addr:?}");
            loop {
                tokio::select! {
                    new_definition = http_definition_tx.recv() => {
                        panic!("new definition");
                    },
                    connection = listener.accept() => {
                        panic!("new connection")
                    }
                }
            }

            unimplemented!("oops, time to start http server: {}", server_function.as_string());
        }
    }
}

impl DefinitionsMonitor {
    pub fn new(
        runtime: Arc<FxRuntime>,
        config: &ServerConfig,
        http_definition_tx: mpsc::Sender<HttpListenerDefinition>,
    ) -> Self {
        Self {
            runtime,
            functions_directory: config.config_path.as_ref().unwrap().parent().unwrap().join(&config.functions_dir),
            functions_configs: Arc::new(Mutex::new(HashMap::new())),
            http_definition_tx: Arc::new(Mutex::new(http_definition_tx)),
        }
    }

    pub async fn scan_definitions(&self) {
        info!("will scan definitions in {:?}", self.functions_directory);

        let mut definition_http = HttpListenerDefinition {
            function: None,
        };

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

            if !entry.file_name().to_str().unwrap().ends_with(DEFINITION_FILE_SUFFIX) {
                continue;
            }

            let entry_path = entry.path();
            let function_id = self.path_to_function_id(entry_path);
            let function_config = FunctionConfig::load(entry_path).await;
            let prev_function_config = self.functions_configs.lock().await.insert(
                function_id.clone(),
                function_config.clone(),
            ).unwrap_or_else(|| EMPTY_FUNCTION_CONFIG.clone());

            self.apply_config(
                function_id,
                prev_function_config,
                function_config.clone(),
                &mut definition_http,
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
            let metadata = tokio::fs::metadata(&path).await.unwrap();
            if !metadata.is_file() {
                continue;
            }

            if !path.file_name().unwrap().to_str().unwrap().ends_with(DEFINITION_FILE_SUFFIX) {
                continue;
            }

            let function_id = self.path_to_function_id(&path);
            let function_config = FunctionConfig::load(&path).await;
            let prev_function_config = self.functions_configs.lock().await.insert(
                function_id.clone(),
                function_config.clone(),
            ).unwrap_or_else(|| EMPTY_FUNCTION_CONFIG.clone());

            self.apply_config(
                function_id,
                prev_function_config,
                function_config,
                &mut definition_http
            ).await;
        }
    }

    async fn apply_config(
        &self,
        function_id: FunctionId,
        prev_config: FunctionConfig,
        new_config: FunctionConfig,
        definition_http: &mut HttpListenerDefinition,
    ) {
        info!("applying config for {:?}", function_id.as_string());

        fn extract_http(config: &FunctionConfig) -> bool {
            config.triggers.as_ref()
                .map(|triggers| !triggers.http.is_empty())
                .unwrap_or(false)
        }

        let prev_http = extract_http(&prev_config);
        let new_http = extract_http(&new_config);

        if prev_http != new_http {
            if new_http {
                if let Some(existing_handler) = definition_http.function.as_ref() {
                    if existing_handler != &function_id {
                        panic!("http listener already set to a different function: {:?}", existing_handler.as_string());
                    } else {
                        // same listener as current, nothing to do
                    }
                } else {
                    // no http listener configured, let's set one
                    definition_http.function = Some(function_id.clone());
                    self.http_definition_tx.lock().await.send(definition_http.clone()).await.unwrap();
                }
            } else {
                if let Some(existing_handler) = definition_http.function.as_ref() {
                    if existing_handler != &function_id {
                        // existing handler set to a different function, nothing to do
                    } else {
                        definition_http.function = None;
                        self.http_definition_tx.lock().await.send(definition_http.clone()).await.unwrap();
                    }
                } else {
                    // no http listener configured, nothing to do
                }
            }
        }

        // TODO: update definitions and send event
    }

    fn path_to_function_id(&self, path: &Path) -> FunctionId {
        let function_id = path.strip_prefix(&self.functions_directory).unwrap().to_str().unwrap();
        let function_id = &function_id[0..function_id.len() - DEFINITION_FILE_SUFFIX.len()];
        FunctionId::new(function_id)
    }
}
