use {
    std::collections::HashMap,
    tokio::sync::oneshot,
    crate::{
        function::{FunctionId, FunctionDeploymentId},
        definitions::{
            triggers::FunctionHttpListener,
            bindings::{BlobBindingConfig, SqlBindingConfig, FunctionBindingConfig},
        },
        triggers::http::FetchRequestHeader,
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
        bindings_functions: HashMap<String, FunctionBindingConfig>,
    },
    FunctionInvoke {
        function_id: FunctionId,
        header: FetchRequestHeader,
        response_tx: flume::Sender<()>,
    },
}
