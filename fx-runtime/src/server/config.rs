use {
    std::path::{PathBuf, Path},
    tokio::fs,
    serde::Deserialize,
};

#[derive(Deserialize)]
pub struct ServerConfig {
    #[serde(skip_deserializing)]
    pub config_path: Option<PathBuf>,

    pub functions_dir: String,
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

    pub triggers: Option<FunctionTriggersConfig>,

    pub sql: Option<Vec<SqlBindingConfig>>,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionTriggersConfig {
    pub http: Vec<FunctionHttpEndpointConfig>,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct FunctionHttpEndpointConfig {
    pub handler: String,
}

#[derive(Deserialize, Debug, Clone, Eq, PartialEq)]
pub struct SqlBindingConfig {
    pub id: String,
    pub path: String,
}

impl FunctionConfig {
    pub async fn load(file_path: PathBuf) -> Self {
        let mut config: Self = serde_yml::from_slice(&fs::read(&file_path).await.unwrap()).unwrap();
        config.config_path = Some(file_path);
        config
    }
}
