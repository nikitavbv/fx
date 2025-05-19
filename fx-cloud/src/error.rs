use {
    thiserror::Error,
    fx_core::FxExecutionError,
};

#[derive(Error, Debug, Eq, PartialEq)]
pub enum FxCloudError {
    #[error("service not found")]
    ServiceNotFound,

    #[error("internal storage error: {reason}")]
    StorageInternalError { reason: String },

    #[error("internal service error: {reason}")]
    ServiceInternalError { reason: String },

    #[error("service executino error: {error:?}")]
    ServiceExecutionError { error: FxExecutionError },

    #[error("compilation error: {reason}")]
    CompilationError { reason: String },

    #[error("rpc handler not defined")]
    RpcHandlerNotDefined,

    #[error("rcp handler has incompatible type")]
    RpcHandlerIncompatibleType,

    #[error("storage does not contain code for this module")]
    ModuleCodeNotFound,

    #[error("configuration error")]
    ConfigurationError { reason: String },

    #[error("cron error: {reason}")]
    CronError { reason: String },

    #[error("streaming error: {reason}")]
    StreamingError { reason: String }
}
