use {
    std::{path::PathBuf, sync::Arc},
    serde::Deserialize,
    thiserror::Error,
    tokio::{io, fs},
    crate::effects::logs::BoxLogger,
};

#[derive(Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(skip_deserializing)]
    pub config_path: Option<PathBuf>,

    #[serde(default)]
    pub server: HttpServerConfig,

    pub functions_dir: String,
    pub cron_data_path: Option<String>,
    pub blob: Option<BlobConfig>,

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

#[derive(Deserialize, Clone, Default)]
pub struct HttpServerConfig {
    #[serde(default)]
    pub port: ServerPort,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ServerPort {
    pub value: u16,
}

impl Default for ServerPort {
    fn default() -> Self {
        Self { value: 8080 }
    }
}

#[derive(Deserialize, Clone)]
pub struct BlobConfig {
    pub path: PathBuf,
}

impl Default for BlobConfig {
    fn default() -> Self {
        Self {
            path: "/var/lib/fx/blob".parse().unwrap(),
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(tag = "type")]
pub enum LoggerConfig {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "noop")]
    Noop,
    #[serde(rename = "http")]
    HttpLogger {
        endpoint: String,
    },
    #[serde(skip)]
    Custom(Arc<BoxLogger>),
}

#[derive(Deserialize, Clone)]
pub struct IntrospectionConfig {
    pub enabled: bool,
}

#[derive(Deserialize, Clone)]
pub struct FunctionConfig {
    #[serde(skip_deserializing)]
    pub config_path: Option<PathBuf>,

    pub code: Option<FunctionCodeConfig>,

    pub env: Option<Vec<EnvVariableConfig>>,
    pub logger: Option<LoggerConfig>,

    pub triggers: Option<FunctionTriggersConfig>,
    pub bindings: Option<FunctionBindingsConfig>,
}

impl FunctionConfig {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path: Some(config_path),
            code: None,
            env: None,
            logger: None,
            triggers: None,
            bindings: None,
        }
    }

    pub fn with_code_inline(mut self, code: Vec<u8>) -> Self {
        self.code = Some(FunctionCodeConfig::Inline(code));
        self
    }

    pub fn with_env(mut self, id: String, value: String) -> Self {
        if self.env.is_none() {
            self.env = Some(Vec::new());
        }

        self.env.as_mut().unwrap().push(EnvVariableConfig {
            id,
            source: EnvValueSource::Value(value),
        });

        self
    }

    pub fn with_trigger_http(mut self, host: Option<String>) -> Self {
        if self.triggers.is_none() {
            self.triggers = Some(FunctionTriggersConfig::new());
        }

        self.triggers = self.triggers.map(|v| v.with_http(host));

        self
    }

    pub fn with_trigger_cron(mut self, id: String, schedule: String) -> Self {
        if self.triggers.is_none() {
            self.triggers = Some(FunctionTriggersConfig::new());
        }

        self.triggers = self.triggers.map(|v| v.with_cron(id, schedule));

        self
    }

    /// Used to configure server externally, for example, in integration tests.
    #[allow(dead_code)]
    pub fn with_binding_blob(mut self, id: String, bucket: String) -> Self {
        if self.bindings.is_none() {
            self.bindings = Some(FunctionBindingsConfig::new());
        }

        self.bindings = self.bindings.map(|v| v.with_blob(id, bucket));

        self
    }

    pub fn with_binding_sql(self, id: String, path: String) -> Self {
        self.with_binding_sql_config(SqlBindingConfig { id, path, busy_timeout_ms: None })
    }

    pub fn with_binding_sql_config(mut self, config: SqlBindingConfig) -> Self {
        if self.bindings.is_none() {
            self.bindings = Some(FunctionBindingsConfig::new());
        }

        self.bindings = self.bindings.map(|v| v.with_sql(config));

        self
    }

    pub fn with_binding_kv(mut self, id: String, namespace: String) -> Self {
        if self.bindings.is_none() {
            self.bindings = Some(FunctionBindingsConfig::new());
        }

        self.bindings = self.bindings.map(|v| v.with_kv(id, namespace));

        self
    }

    pub fn with_binding_function(mut self, config: FunctionBindingConfig) -> Self {
        if self.bindings.is_none() {
            self.bindings = Some(FunctionBindingsConfig::new());
        }

        self.bindings = self.bindings.map(|v| v.with_function(config));

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

    pub fn with_http(mut self, host: Option<String>) -> Self {
        self.http = Some(vec![FunctionHttpEndpointConfig::new(host)]);
        self
    }

    pub fn with_cron(mut self, id: String, schedule: String) -> Self {
        self.cron = Some(vec![FunctionCronTriggerConfig::new(id, schedule)]);
        self
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionHttpEndpointConfig {
    pub host: Option<String>,
}

impl FunctionHttpEndpointConfig {
    pub fn new(host: Option<String>) -> Self {
        Self {
            host,
        }
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionCronTriggerConfig {
    pub id: String,
    pub schedule: String,
}

impl FunctionCronTriggerConfig {
    fn new(id: String, schedule: String) -> Self {
        Self {
            id,
            schedule,
        }
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionRabbitmqTriggerConfig {
    pub handler: String,
    pub queue: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionBindingsConfig {
    pub sql: Option<Vec<SqlBindingConfig>>,
    pub blob: Option<Vec<BlobBindingConfig>>,
    pub kv: Option<Vec<KvBindingConfig>>,
    pub functions: Option<Vec<FunctionBindingConfig>>,
}

impl FunctionBindingsConfig {
    pub fn new() -> Self {
        Self {
            sql: None,
            blob: None,
            kv: None,
            functions: None,
        }
    }

    pub fn with_blob(mut self, id: String, bucket: String) -> Self {
        if self.blob.is_none() {
            self.blob = Some(Vec::new());
        }

        self.blob.as_mut().unwrap().push(BlobBindingConfig {
            id,
            bucket,
        });

        self
    }

    pub fn with_sql(mut self, config: SqlBindingConfig) -> Self {
        if self.sql.is_none() {
            self.sql = Some(Vec::new());
        }

        self.sql.as_mut().unwrap().push(config);

        self
    }

    pub fn with_kv(mut self, id: String, namespace: String) -> Self {
        if self.kv.is_none() {
            self.kv = Some(Vec::new());
        }

        self.kv.as_mut().unwrap().push(KvBindingConfig { id, namespace });

        self
    }

    pub fn with_function(mut self, config: FunctionBindingConfig) -> Self {
        if self.functions.is_none() {
            self.functions = Some(Vec::new());
        }

        self.functions.as_mut().unwrap().push(config);

        self
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct SqlBindingConfig {
    pub id: String,
    pub path: String,
    #[serde(skip)]
    pub busy_timeout_ms: Option<u64>,
}

impl SqlBindingConfig {
    pub fn new(id: String, path: String) -> Self {
        Self {
            id,
            path,
            busy_timeout_ms: None,
        }
    }

    pub fn with_busy_timeout_ms(mut self, busy_timeout_ms: u64) -> Self {
        self.busy_timeout_ms = Some(busy_timeout_ms);
        self
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct BlobBindingConfig {
    pub id: String,
    pub bucket: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct KvBindingConfig {
    pub id: String,
    pub namespace: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionBindingConfig {
    pub id: String,
    pub function_id: String,
    pub host: Option<String>,
}

impl FunctionBindingConfig {
    pub fn new(id: String, function_id: String, host: Option<String>) -> Self {
        Self {
            id,
            function_id,
            host,
        }
    }
}

#[derive(Error, Debug)]
pub(crate) enum FunctionConfigLoadError {
    #[error("failed to read config file: {0:?}")]
    FailedToRead(io::Error),
    #[error("failed to parse config file: {0:?}")]
    FailedToParse(serde_yml::Error),
}

impl FunctionConfig {
    pub(crate) async fn load(file_path: PathBuf) -> Result<Self, FunctionConfigLoadError> {
        let mut config: Self = serde_yml::from_slice(
            &fs::read(&file_path).await
                .map_err(|err| FunctionConfigLoadError::FailedToRead(err))?
        ).map_err(|err| FunctionConfigLoadError::FailedToParse(err))?;
        config.config_path = Some(file_path);
        Ok(config)
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct EnvVariableConfig {
    pub id: String,
    #[serde(flatten)]
    pub source: EnvValueSource,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EnvValueSource {
    Value(String),
    File(PathBuf),
}
