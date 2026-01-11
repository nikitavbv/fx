use {
    thiserror::Error,
    fx_common::FxExecutionError,
};

/// Error that may be returned when function is invoked by the runtime.
#[derive(Error, Debug)]
pub enum FunctionInvokeError {
    /// Failed to invoke function because of internal error in runtime implementation.
    /// Should never happen. If you get this error, there is a bug somewhere in the
    /// fx runtime implementation.
    #[error("runtime internal error: {0:?}")]
    RuntimeError(#[from] FunctionInvokeInternalRuntimeError),
}

/// Failed to invoke function because of internal error in runtime implementation.
/// Should never happen. If you get this error, there is a bug somewhere in the
/// fx runtime implementation.
#[derive(Error, Debug)]
pub enum FunctionInvokeInternalRuntimeError {
    #[error("failed to lock execution contexts: {reason:?}")]
    ExecutionContextsFailedToLock {
        reason: String,
    }
}

// legacy errors:
#[derive(Error, Debug, Eq, PartialEq)]
pub enum FxRuntimeError {
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
