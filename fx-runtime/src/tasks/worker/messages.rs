use {
    std::collections::HashMap,
    tokio::sync::oneshot,
    thiserror::Error,
    crate::{
        function::{FunctionId, FunctionDeploymentId},
        definitions::{
            triggers::FunctionHttpListener,
            bindings::{BlobBindingConfig, SqlBindingConfig, FunctionBindingConfig, KvBindingConfig},
        },
        triggers::http::{FetchRequestHeader, FunctionResponse},
        resources::serialize::SerializedFunctionResource,
    },
};

pub(crate) enum WorkerMessage {
    RemoveFunction {
        function_id: FunctionId,
        on_ready: Option<oneshot::Sender<()>>,
    },
    FunctionDeploy {
        function_id: FunctionId,
        deployment_id: FunctionDeploymentId,
        module: wasmtime::Module,

        limit_memory_bytes: Option<usize>,

        http_listeners: Vec<FunctionHttpListener>,

        env: HashMap<String, String>,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
        bindings_kv: HashMap<String, KvBindingConfig>,
        bindings_functions: HashMap<String, FunctionBindingConfig>,
    },
    FunctionInvoke {
        function_id: FunctionId,
        header: FetchRequestHeader,
        response_tx: oneshot::Sender<Result<(), FunctionInvokeError>>,
    },
}

pub(crate) enum WorkerLocalMessage {
    FunctionInvoke {
        function_id: FunctionId,
        header: FetchRequestHeader,
        response_tx: async_unsync::oneshot::Sender<SerializedFunctionResource<FunctionResponse>>,
    }
}

#[derive(Debug, Error)]
pub(crate) enum FunctionInvokeError {
    #[error("function with this id is not found")]
    NotFound,
}
