use {
    std::collections::HashMap,
    tokio::sync::oneshot,
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

        http_listeners: Vec<FunctionHttpListener>,

        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
        bindings_kv: HashMap<String, KvBindingConfig>,
        bindings_functions: HashMap<String, FunctionBindingConfig>,
    },
    FunctionInvoke {
        function_id: FunctionId,
        header: FetchRequestHeader,
        response_tx: oneshot::Sender<()>,
    },
}

pub(crate) enum WorkerLocalMessage {
    FunctionInvoke {
        function_id: FunctionId,
        header: FetchRequestHeader,
        response_tx: async_unsync::oneshot::Sender<SerializedFunctionResource<FunctionResponse>>,
    }
}
