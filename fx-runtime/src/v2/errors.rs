use thiserror::Error;

/// Error that occured while polling function future
#[derive(Debug, Error)]
pub enum FunctionFuturePollError {
    /// Function panicked when future poll was callled
    #[error("function panicked")]
    FunctionPanicked,
}

/// Error that occured while running FunctionFuture
#[derive(Debug, Error)]
pub enum FunctionFutureError {
    /// Function panicked while it was running
    #[error("function panicked")]
    FunctionPanicked,
}

#[derive(Debug, Error)]
pub enum FunctionDeploymentHandleRequestError {
    /// Function panicked while handling request
    #[error("function panicked")]
    FunctionPanicked,
}
