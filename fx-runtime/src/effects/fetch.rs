use {
    thiserror::Error,
    crate::{
        resources::resource::HttpBodyResourceKey,
        tasks::worker::FunctionInvokeError,
    },
};

#[derive(Error, Debug)]
pub(crate) enum FetchResultError {
    #[error("connection failed")]
    ConnectionFailed,
    #[error("connection timeout")]
    ConnectionTimeout,
    #[error("response timeout")]
    ResponseTimeout,

    #[error("rpc target function not found")]
    FunctionNotFound,
    #[error("failed to create instance of rpc target function")]
    FunctionInstantiationError,
    #[error("rpc target function panicked")]
    FunctionPanicked,
    #[error("rpc target function stopped execution with unknown wasm error")]
    FunctionCrashed,
    #[error("rpc target function is busy handling other requests and cannot accept a new one")]
    FunctionBusy,
    #[error("rpc target function returned incorrect response")]
    FunctionIncorrectResponse,
    #[error("runtime is being shut down, new requests are not accepted")]
    RuntimeShutdown,
    #[error("internal runtime assertion error")]
    InternalRuntimeAssertionError,
}

impl From<FunctionInvokeError> for FetchResultError {
    fn from(err: FunctionInvokeError) -> Self {
        match err {
            FunctionInvokeError::NotFound => Self::FunctionNotFound,
            FunctionInvokeError::FunctionPanicked => Self::FunctionPanicked,
            FunctionInvokeError::FunctionCrashed => Self::FunctionCrashed,
            FunctionInvokeError::FunctionBusy => Self::FunctionBusy,
            FunctionInvokeError::FunctionIncorrectResponse => Self::FunctionIncorrectResponse,
            FunctionInvokeError::InternalRuntimeAssertionError => Self::InternalRuntimeAssertionError,
            FunctionInvokeError::FunctionInstantiationError => Self::FunctionInstantiationError,
        }
    }
}

impl From<crate::tasks::worker::local_worker_controller::invoke_function::FunctionInvokeError> for FetchResultError {
    fn from(err: crate::tasks::worker::local_worker_controller::invoke_function::FunctionInvokeError) -> Self {
        use crate::tasks::worker::local_worker_controller::invoke_function::FunctionInvokeError as SourceError;
        match err {
            SourceError::NotFound => Self::FunctionNotFound,
            SourceError::FunctionPanicked => Self::FunctionPanicked,
            SourceError::FunctionCrashed => Self::FunctionCrashed,
            SourceError::FunctionBusy => Self::FunctionBusy,
            SourceError::RuntimeShutdown => Self::RuntimeShutdown,
            SourceError::FunctionIncorrectResponse => Self::FunctionIncorrectResponse,
            SourceError::InternalRuntimeAssertionError => Self::InternalRuntimeAssertionError,
            SourceError::FunctionInstantiationError => Self::FunctionInstantiationError,
        }
    }
}

pub(crate) struct FetchResultWithBodyResource {
    pub(crate) parts: http::response::Parts,
    pub(crate) body: HttpBodyResourceKey,
}

impl FetchResultWithBodyResource {
    pub fn new(parts: http::response::Parts, body: HttpBodyResourceKey) -> Self {
        Self { parts, body }
    }
}

#[derive(Error, Debug)]
pub enum HttpStreamError {
    #[error("failed to read fetch response stream")]
    FetchResponseStreamError(reqwest::Error),
    #[error("failed to read http request body stream")]
    RequestBodyStreamError,
}
