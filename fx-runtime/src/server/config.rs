use {
    std::{path::{PathBuf, Path}, sync::Arc},
    tokio::{fs, io},
    serde::Deserialize,
    thiserror::Error,
    crate::runtime::logs::BoxLogger,
};

#[derive(Deserialize)]
pub struct ServerConfig {
    #[serde(skip_deserializing)]
    pub config_path: Option<PathBuf>,

    pub functions_dir: String,
    pub cron_data_path: Option<String>,

    pub logger: Option<LoggerConfig>,
    pub introspection: Option<IntrospectionConfig>,
}

impl ServerConfig {
    pub fn load(file_path: PathBuf) -> Self {
        let mut config: Self = serde_yml::from_slice(&std::fs::read(&file_path).unwrap()).unwrap();
        config.config_path = Some(file_path);
        config
    }
}

#[derive(Deserialize, Clone)]
#[serde(tag = "type")]
pub enum LoggerConfig {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "noop")]
    Noop,
    #[serde(skip)]
    Custom(Arc<BoxLogger>),
}

#[derive(Deserialize)]
pub struct IntrospectionConfig {
    pub enabled: bool,
}

#[derive(Deserialize, Clone)]
pub struct FunctionConfig {
    #[serde(skip_deserializing)]
    pub config_path: Option<PathBuf>,

    pub code: Option<FunctionCodeConfig>,

    pub triggers: Option<FunctionTriggersConfig>,
    pub bindings: Option<FunctionBindingsConfig>,
    pub logger: Option<LoggerConfig>,
}

impl FunctionConfig {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path: Some(config_path),
            code: None,
            triggers: None,
            bindings: None,
            logger: None,
        }
    }

    pub fn with_code_inline(mut self, code: Vec<u8>) -> Self {
        self.code = Some(FunctionCodeConfig::Inline(code));
        self
    }

    pub fn with_trigger_http(mut self) -> Self {
        if self.triggers.is_none() {
            self.triggers = Some(FunctionTriggersConfig::new());
        }

        self.triggers = self.triggers.map(|v| v.with_http());

        self
    }

    pub fn with_binding_kv(mut self, id: String, path: String) -> Self {
        if self.bindings.is_none() {
            self.bindings = Some(FunctionBindingsConfig::new());
        }

        self.bindings = self.bindings.map(|v| v.with_kv(id, path));

        self
    }

    /// Used to configure server externally, for example, in integration tests.
    #[allow(dead_code)]
    pub fn with_binding_blob(mut self, id: String, path: String) -> Self {
        if self.bindings.is_none() {
            self.bindings = Some(FunctionBindingsConfig::new());
        }

        self.bindings = self.bindings.map(|v| v.with_blob(id, path));

        self
    }

    pub fn with_binding_sql(mut self, id: String, path: String) -> Self {
        if self.bindings.is_none() {
            self.bindings = Some(FunctionBindingsConfig::new());
        }

        self.bindings = self.bindings.map(|v| v.with_sql(id, path));

        self
    }

    pub fn with_binding_rpc(mut self, id: String, path: String) -> Self {
        if self.bindings.is_none() {
            self.bindings = Some(FunctionBindingsConfig::new());
        }

        self.bindings = self.bindings.map(|v| v.with_rpc(id, path));

        self
    }

    pub fn with_logger(mut self, logger: LoggerConfig) -> Self {
        self.logger = Some(logger);
        self
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(untagged)]
pub enum FunctionCodeConfig {
    Path(String),
    Inline(Vec<u8>),
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionTriggersConfig {
    pub http: Option<Vec<FunctionHttpEndpointConfig>>,
    pub cron: Option<Vec<FunctionCronTriggerConfig>>,
}

impl FunctionTriggersConfig {
    pub fn new() -> Self {
        Self {
            http: None,
            cron: None,
        }
    }

    pub fn with_http(mut self) -> Self {
        self.http = Some(vec![FunctionHttpEndpointConfig::new()]);
        self
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionHttpEndpointConfig {
    pub host: Option<String>,
}

impl FunctionHttpEndpointConfig {
    pub fn new() -> Self {
        Self {
            host: None,
        }
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionCronTriggerConfig {
    pub id: String,
    pub handler: String,
    pub schedule: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionRabbitmqTriggerConfig {
    pub handler: String,
    pub queue: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionBindingsConfig {
    pub sql: Option<Vec<SqlBindingConfig>>,
    pub rpc: Option<Vec<RpcBindingConfig>>,
    pub blob: Option<Vec<BlobBindingConfig>>,
    pub kv: Option<Vec<KvBindingConfig>>,
}

impl FunctionBindingsConfig {
    pub fn new() -> Self {
        Self {
            sql: None,
            rpc: None,
            kv: None,
            blob: None,
        }
    }

    pub fn with_blob(mut self, id: String, path: String) -> Self {
        if self.blob.is_none() {
            self.blob = Some(Vec::new());
        }

        self.blob.as_mut().unwrap().push(BlobBindingConfig {
            id,
            path,
        });

        self
    }

    pub fn with_kv(mut self, id: String, path: String) -> Self {
        if self.kv.is_none() {
            self.kv = Some(Vec::new());
        }

        self.kv.as_mut().unwrap().push(KvBindingConfig {
            id,
            path
        });

        self
    }

    pub fn with_sql(mut self, id: String, path: String) -> Self {
        if self.sql.is_none() {
            self.sql = Some(Vec::new());
        }

        self.sql.as_mut().unwrap().push(SqlBindingConfig {
            id,
            path,
        });

        self
    }

    pub fn with_rpc(mut self, id: String, path: String) -> Self {
        if self.rpc.is_none() {
            self.rpc = Some(Vec::new());
        }

        self.rpc.as_mut().unwrap().push(RpcBindingConfig { id, function: path });

        self
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct SqlBindingConfig {
    pub id: String,
    pub path: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct RpcBindingConfig {
    pub id: String,
    pub function: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct KvBindingConfig {
    pub id: String,
    pub path: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct BlobBindingConfig {
    pub id: String,
    pub path: String,
}

#[derive(Error, Debug)]
pub(crate) enum FunctionConfigLoadError {
    #[error("failed to read config file: {0:?}")]
    FailedToRead(io::Error),
}

impl FunctionConfig {
    pub async fn load(file_path: PathBuf) -> Result<Self, FunctionConfigLoadError> {
        let mut config: Self = serde_yml::from_slice(
            &fs::read(&file_path).await
                .map_err(|err| FunctionConfigLoadError::FailedToRead(err))?
        ).unwrap();
        config.config_path = Some(file_path);
        Ok(config)
    }
}
