use {
    std::collections::HashMap,
    tokio::sync::oneshot,
    crate::{
        function::{FunctionId, FunctionDeploymentId},
        definitions::{
            triggers::FunctionHttpListener,
            bindings::{BlobBindingConfig, SqlBindingConfig},
        },
    },
};

#[derive(Debug)]
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
    },
}
