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

    #[error("failed to init execution context: {reason}")]
    ExecutionContextInitError { reason: String },

    #[error("execution context runtime error: {reason}")]
    ExecutionContextRuntimeError { reason: String },

    #[error("service execution error: {error:?}")]
    ServiceExecutionError { error: FxExecutionError },

    #[error("compilation error: {reason}")]
    CompilationError { reason: String },

    #[error("rpc handler not defined")]
    RpcHandlerNotDefined,

    #[error("rcp handler has incompatible type")]
    RpcHandlerIncompatibleType,

    #[error("storage does not contain code for this module")]
    ModuleCodeNotFound,

    #[error("definition error")]
    DefinitionError { reason: String },

    #[error("configuration error")]
    ConfigurationError { reason: String },

    #[error("cron error: {reason}")]
    CronError { reason: String },

    #[error("streaming error: {reason}")]
    StreamingError { reason: String },

    #[error("stream not found")]
    StreamNotFound,

    #[error("serialization error")]
    SerializationError { reason: String },
}

#[derive(Error, Debug)]
pub enum LoggerError {
    #[error("failed to create logger: {reason:?}")]
    FailedToCreate { reason: String },
}

#[derive(Error, Debug)]
pub enum KVWatchError {
    #[error("failed to init watch: {reason:?}")]
    FailedToInit { reason: String },

    #[error("failed to handle event: {reason:?}")]
    EventHandling { reason: String },
}
