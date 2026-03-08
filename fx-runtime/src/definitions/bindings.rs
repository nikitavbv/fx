use {
    std::path::PathBuf,
    tokio::time::Duration,
    crate::function::FunctionId,
};

#[derive(Debug, Clone)]
pub(crate) struct SqlBindingConfig {
    pub(crate) connection_id: String,
    pub(crate) location: SqlBindingConfigLocation,
    pub(crate) busy_timeout: Option<Duration>,
}

#[derive(Debug, Clone)]
pub(crate) enum SqlBindingConfigLocation {
    InMemory(String),
    Path(PathBuf),
}

#[derive(Debug, Clone)]
pub(crate) struct BlobBindingConfig {
    pub(crate) bucket: String,
}

#[derive(Debug, Clone)]
pub(crate) struct KvBindingConfig {
    pub(crate) namespace: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FunctionBindingConfig {
    pub(crate) function_id: FunctionId,
}
