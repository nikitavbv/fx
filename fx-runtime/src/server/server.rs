use {
    std::{sync::Arc, path::PathBuf, collections::HashMap},
    tokio::sync::{Mutex, RwLock},
    tracing::{info, warn},
    walkdir::WalkDir,
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
    listener_definitions: Arc<RwLock<ListenerDefinitions>>,
}

struct DefinitionsMonitor {
    runtime: Arc<FxRuntime>,
    listener_definitions: Arc<RwLock<ListenerDefinitions>>,
    functions_directory: PathBuf,
    functions_configs: Arc<Mutex<HashMap<FunctionId, FunctionConfig>>>,
}

struct ListenerDefinitions {
    http: Option<Arc<HttpListenerDefinition>>,
}

struct HttpListenerDefinition {
    function_id: FunctionId,
}

impl FxServer {
    pub fn new(config: ServerConfig, runtime: FxRuntime) -> Self {
        let runtime = Arc::new(runtime);
        let listener_definitions = Arc::new(RwLock::new(ListenerDefinitions::new()));
        let definitions_monitor = DefinitionsMonitor::new(runtime.clone(), listener_definitions.clone(), &config);

        Self {
            runtime,
            definitions_monitor,
            listener_definitions,
        }
    }

    pub async fn serve(&self) {
        self.definitions_monitor.scan_definitions().await;

        unimplemented!("serving stuff is not implemented yet")
    }
}

impl DefinitionsMonitor {
    pub fn new(runtime: Arc<FxRuntime>, listener_definitions: Arc<RwLock<ListenerDefinitions>>, config: &ServerConfig) -> Self {
        Self {
            runtime,
            listener_definitions,
            functions_directory: config.config_path.as_ref().unwrap().parent().unwrap().join(&config.functions_dir),
            functions_configs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn scan_definitions(&self) {
        info!("will scan definitions in {:?}", self.functions_directory);

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

            let function_id = entry_path.strip_prefix(root).unwrap().to_str().unwrap();
            let function_id = &function_id[0..function_id.len() - DEFINITION_FILE_SUFFIX.len()];
            let function_id = FunctionId::new(function_id);
            let function_config = FunctionConfig::load(entry_path).await;
            let prev_function_config = self.functions_configs.lock().await.get(&function_id).unwrap_or(&EMPTY_FUNCTION_CONFIG).clone();

            self.apply_config(function_id, prev_function_config, function_config);
        }

        unimplemented!("definitions scanning is not implemented yet")
    }

    fn apply_config(&self, function_id: FunctionId, prev_config: FunctionConfig, new_config: FunctionConfig) {
        info!("applying config for {:?}", function_id.as_string());
        // TODO: update definitions and send event
    }
}

impl ListenerDefinitions {
    fn new() -> Self {
        Self {
            http: None,
        }
    }
}
