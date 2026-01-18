use {
    std::{sync::Arc, path::PathBuf},
    tracing::info,
    crate::{
        runtime::FxRuntime,
        server::config::ServerConfig,
    },
};

pub struct FxServer {
    runtime: Arc<FxRuntime>,
    definitions_monitor: DefinitionsMonitor,
}

struct DefinitionsMonitor {
    runtime: Arc<FxRuntime>,
    functions_directory: PathBuf,
}

impl FxServer {
    pub fn new(config: ServerConfig, runtime: FxRuntime) -> Self {
        let runtime = Arc::new(runtime);
        let definitions_monitor = DefinitionsMonitor::new(runtime.clone(), &config);

        Self {
            runtime,
            definitions_monitor,
        }
    }

    pub async fn serve(&self) {
        self.definitions_monitor.scan_definitions().await;

        unimplemented!("serving stuff is not implemented yet")
    }
}

impl DefinitionsMonitor {
    pub fn new(runtime: Arc<FxRuntime>, config: &ServerConfig) -> Self {
        Self {
            runtime,
            functions_directory: config.config_path.as_ref().unwrap().parent().unwrap().join(&config.functions_dir),
        }
    }

    pub async fn scan_definitions(&self) {
        info!("will scan definitions in {:?}", self.functions_directory);
        unimplemented!("definitions scanning is not implemented yet")
    }
}
