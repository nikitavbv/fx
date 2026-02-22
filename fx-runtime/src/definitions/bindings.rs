#[derive(Debug, Clone)]
pub(crate) struct SqlBindingConfig {
    pub(crate) connection_id: String,
    pub(crate) location: SqlBindingConfigLocation,
    pub(crate) busy_timeout: Option<Duration>,
}

#[derive(Debug, Clone)]
enum SqlBindingConfigLocation {
    InMemory(String),
    Path(PathBuf),
}

#[derive(Debug, Clone)]
struct BlobBindingConfig {
    storage_directory: PathBuf,
}
