use {
    std::{collections::HashMap, rc::Rc},
    tokio::sync::oneshot,
    thiserror::Error,
    crate::{
        function::{
            FunctionId,
            FunctionDeploymentId,
            deployment::FunctionDeploymentHandleRequestError,
            instance::FunctionInstance,
        },
        definitions::{
            triggers::FunctionHttpListener,
            bindings::{BlobBindingConfig, SqlBindingConfig, FunctionBindingConfig, KvBindingConfig},
        },
        triggers::http::{FetchRequestHeader, HttpBody},
        resources::FunctionResourceId,
    },
};

pub(crate) enum WorkerMessage {
    RemoveFunction {
        function_id: FunctionId,
        on_ready: Option<oneshot::Sender<()>>,
    },
    FunctionDeploy(Box<FunctionDeployMessage>),
    FunctionInvoke(Box<FunctionInvokeMessage>),
}

pub(crate) struct FunctionDeployMessage {
    pub(crate) function_id: FunctionId,
    pub(crate) deployment_id: FunctionDeploymentId,
    pub(crate) module: wasmtime::Module,

    pub(crate) limit_memory_bytes: Option<usize>,

    pub(crate) http_listeners: Vec<FunctionHttpListener>,

    pub(crate) env: HashMap<String, String>,
    pub(crate) bindings_sql: HashMap<String, SqlBindingConfig>,
    pub(crate) bindings_blob: HashMap<String, BlobBindingConfig>,
    pub(crate) bindings_kv: HashMap<String, KvBindingConfig>,
    pub(crate) bindings_functions: HashMap<String, FunctionBindingConfig>,
}

pub(crate) struct FunctionInvokeMessage {
    pub(crate) function_id: FunctionId,
    pub(crate) header: FetchRequestHeader,
    pub(crate) response_tx: oneshot::Sender<Result<(), FunctionInvokeError>>,
}

pub(crate) enum WorkerLocalMessage {
    FunctionInvoke(Box<LocalFunctionInvokeMessage>),
    FunctionBytesDrop {
        instance: Rc<FunctionInstance>,
        bytes_resource_id: FunctionResourceId,
    },
}

pub(crate) struct LocalFunctionInvokeMessage {
    pub(crate) function_id: FunctionId,
    pub(crate) header: FetchRequestHeader,
    pub(crate) response_tx: async_unsync::oneshot::Sender<Result<http::Response<HttpBody>, FunctionInvokeError>>,
}

#[derive(Debug, Error)]
pub(crate) enum FunctionInvokeError {
    #[error("function with this id is not found")]
    NotFound,

    #[error("function panicked during execution")]
    FunctionPanicked,

    #[error("function stopped execution with unknown wasm trap")]
    FunctionCrashed,

    #[error("function is busy handling other requests and cannot accept new requests")]
    FunctionBusy,

    #[error("function returned incorrect response")]
    FunctionIncorrectResponse,

    #[error("internal runtime assertion error")]
    InternalRuntimeAssertionError,

    #[error("failed to create function instance")]
    FunctionInstantiationError,
}

impl From<FunctionDeploymentHandleRequestError> for FunctionInvokeError {
    fn from(err: FunctionDeploymentHandleRequestError) -> Self {
        match err {
            FunctionDeploymentHandleRequestError::AssertionError => Self::InternalRuntimeAssertionError,
            FunctionDeploymentHandleRequestError::FunctionBusy => Self::FunctionBusy,
            FunctionDeploymentHandleRequestError::FunctionPanicked => Self::FunctionPanicked,
            FunctionDeploymentHandleRequestError::FunctionCrashed => Self::FunctionCrashed,
            FunctionDeploymentHandleRequestError::FunctionIncorrectResponse => Self::FunctionIncorrectResponse,
            FunctionDeploymentHandleRequestError::FunctionInstantiationError => Self::FunctionInstantiationError,
        }
    }
}
