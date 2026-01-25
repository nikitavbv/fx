use {
    std::path::{PathBuf, Path},
    tokio::{fs, io},
    serde::Deserialize,
    thiserror::Error,
};

#[derive(Deserialize)]
pub struct ServerConfig {
    #[serde(skip_deserializing)]
    pub config_path: Option<PathBuf>,

    pub functions_dir: String,
    pub cron_data_path: Option<String>,

    pub logger: Option<LoggerConfig>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum LoggerConfig {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "noop")]
    Noop,
    #[serde(rename = "rabbitmq")]
    RabbitMq {
        uri: String,
        exchange: String,
    },
}

impl ServerConfig {
    pub async fn load(file_path: PathBuf) -> Self {
        let mut config: Self = serde_yml::from_slice(&fs::read(&file_path).await.unwrap()).unwrap();
        config.config_path = Some(file_path);
        config
    }
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionConfig {
    #[serde(skip_deserializing)]
    pub config_path: Option<PathBuf>,

    pub code: Option<FunctionCodeConfig>,

    pub triggers: Option<FunctionTriggersConfig>,
    pub bindings: Option<FunctionBindingsConfig>,
}

impl FunctionConfig {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path: Some(config_path),
            code: None,
            triggers: None,
            bindings: None,
        }
    }

    pub fn with_code_inline(mut self, code: Vec<u8>) -> Self {
        self.code = Some(FunctionCodeConfig::Inline(code));
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

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionHttpEndpointConfig {
    pub handler: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionCronTriggerConfig {
    pub id: String,
    pub handler: String,
    pub schedule: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionBindingsConfig {
    pub sql: Option<Vec<SqlBindingConfig>>,
    pub rpc: Option<Vec<RpcBindingConfig>>,
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
