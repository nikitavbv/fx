use {
    thiserror::Error,
    fx_common::FxExecutionError,
    crate::runtime::{
        definition::DefinitionError,
        kv::StorageError,
        runtime::CompilerError,
    },
};

/// Error that may be returned when function is invoked by the runtime.
#[derive(Error, Debug)]
pub enum FunctionInvokeError {
    /// Failed to invoke function because of internal error in runtime implementation.
    /// Should never happen. Getting this error means there is a bug somewhere.
    #[error("runtime internal error: {0:?}")]
    RuntimeError(#[from] FunctionInvokeInternalRuntimeError),

    /// Definitions are required in order to create an instance of function, so invocation
    /// will fail if definition failed to load.
    #[error("failed to get definition: {0:?}")]
    DefinitionMissing(DefinitionError),

    /// Function cannot be invoked if runtime failed to load its code
    #[error("failed to load code: {0:?}")]
    CodeFailedToLoad(StorageError),

    /// Function cannot be invoked if runtime could not find its code in storage
    #[error("code not found")]
    CodeNotFound,

    /// Function cannot be invoked if it failed to compile
    #[error("failed to compile: {0:?}")]
    FailedToCompile(CompilerError),
}

/// Failed to invoke function because of internal error in runtime implementation.
/// Should never happen. Getting this error means there is a bug somewhere.
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
