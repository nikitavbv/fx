use {
    std::path::PathBuf,
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
