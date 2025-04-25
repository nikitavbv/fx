use thiserror::Error;

#[derive(Error, Debug)]
pub enum FxCloudError {
    #[error("service not found")]
    ServiceNotFound,

    #[error("internal storage error: {reason}")]
    StorageInternalError { reason: String },

    #[error("internal service error: {reason}")]
    ServiceInternalError { reason: String },

    #[error("compilation error: {reason}")]
    CompilationError { reason: String },
}
