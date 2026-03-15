use {
    std::{rc::Rc, collections::HashMap, pin::Pin},
    thiserror::Error,
    serde::{Serialize, Deserialize},
    crate::{
        effects::logs::LogMessageEvent,
        tasks::{sql::SqlMessage, worker::LocalWorkerController, kv::KvMessage, blob::BlobMessage},
        definitions::bindings::{SqlBindingConfig, BlobBindingConfig, FunctionBindingConfig, KvBindingConfig},
        triggers::http::{FetchRequestHeader, FunctionResponse, FetchRequestBody},
        resources::{
            Resource,
            serialize::{SerializedFunctionResource, SerializableResource},
            future::{FunctionFuture, FunctionUnitFuture},
        },
    },
    super::instance::{FunctionInstanceState, FunctionInstance, FunctionInstanceInitError},
};

/// deployment is a set of FunctionInstances deployed with same configuration
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub(crate) struct FunctionDeploymentId {
    id: u64,
}

impl FunctionDeploymentId {
    pub(crate) fn new(id: u64) -> Self {
        Self { id }
    }
}

pub(crate) struct FunctionDeployment {
    pub(crate) function_id: FunctionId,
    template: FunctionTemplate,
    pub(crate) instance: Rc<FunctionInstance>,
}

impl FunctionDeployment {
    pub async fn new(
        wasmtime: Rc<wasmtime::Engine>,
        limit_memory_bytes: Option<usize>,
        local_worker: LocalWorkerController,
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        kv_tx: flume::Sender<KvMessage>,
        blob_tx: flume::Sender<BlobMessage>,
        function_id: FunctionId,
        module: wasmtime::Module,
        env: HashMap<String, String>,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
        bindings_kv: HashMap<String, KvBindingConfig>,
        bindings_functions: HashMap<String, FunctionBindingConfig>,
    ) -> Result<Self, DeploymentInitError> {
        let mut linker = wasmtime::Linker::<FunctionInstanceState>::new(&wasmtime);

        linker.func_wrap("fx", "fx_log", super::abi::fx_log_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_serialize", super::abi::fx_resource_serialize_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_move_from_host", super::abi::fx_resource_move_from_host_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_drop", super::abi::fx_resource_drop_handler).unwrap();
        linker.func_wrap("fx", "fx_sql_exec", super::abi::fx_sql_exec_handler).unwrap();
        linker.func_wrap("fx", "fx_sql_batch", super::abi::fx_sql_batch_handler).unwrap();
        linker.func_wrap("fx", "fx_sql_migrate", super::abi::fx_sql_migrate_handler).unwrap();
        linker.func_wrap("fx", "fx_future_poll", super::abi::fx_future_poll_handler).unwrap();
        linker.func_wrap("fx", "fx_sleep", super::abi::fx_sleep_handler).unwrap();
        linker.func_wrap("fx", "fx_random", super::abi::fx_random_handler).unwrap();
        linker.func_wrap("fx", "fx_time", super::abi::fx_time_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_put", super::abi::fx_blob_put_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_get", super::abi::fx_blob_get_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_delete", super::abi::fx_blob_delete_handler).unwrap();
        linker.func_wrap("fx", "fx_fetch", super::abi::fx_fetch_handler).unwrap();
        linker.func_wrap("fx", "fx_metrics_counter_register", super::abi::fx_metrics_counter_register_handler).unwrap();
        linker.func_wrap("fx", "fx_metrics_counter_increment", super::abi::fx_metrics_counter_increment_handler).unwrap();
        linker.func_wrap("fx", "fx_stream_frame_read", super::abi::fx_stream_frame_read_handler).unwrap();
        linker.func_wrap("fx", "fx_env_len", super::abi::fx_env_len_handler).unwrap();
        linker.func_wrap("fx", "fx_env_get", super::abi::fx_env_get_handler).unwrap();
        linker.func_wrap("fx", "fx_kv_set", super::abi::fx_kv_set_handler).unwrap();
        linker.func_wrap("fx", "fx_kv_set_nx_px", super::abi::fx_kv_set_nx_px_handler).unwrap();
        linker.func_wrap("fx", "fx_kv_get", super::abi::fx_kv_get_handler).unwrap();
        linker.func_wrap("fx", "fx_kv_delex_ifeq", super::abi::fx_kv_delex_ifeq_handler).unwrap();
        linker.func_wrap("fx", "fx_kv_subscribe", super::abi::fx_kv_subscribe_handler).unwrap();
        linker.func_wrap("fx", "fx_kv_publish", super::abi::fx_kv_publish_handler).unwrap();
        linker.func_wrap("fx", "fx_tasks_background_spawn", super::abi::fx_tasks_background_spawn_handler).unwrap();

        for import in module.imports() {
            if import.module() == "fx" {
                continue;
            }

            if let Some(f) = import.ty().func() {
                linker.func_new(
                    import.module(),
                    import.name(),
                    f.clone(),
                    move |_, _, _| {
                        Err(wasmtime::Error::msg("requested function is not implemented by fx runtime"))
                    }
                ).unwrap();
            }
        }

        let instance_template = linker.instantiate_pre(&module)
            .map_err(|err| {
                if let Some(_) = err.downcast_ref::<wasmtime::UnknownImportError>() {
                    return DeploymentInitError::MissingImport;
                } else {
                    todo!("handle other error: {err:?}")
                }
            })?;

        let template = FunctionTemplate::new(
            wasmtime,
            limit_memory_bytes,
            local_worker,
            logger_tx,
            sql_tx,
            kv_tx,
            blob_tx,
            function_id.clone(),
            instance_template,
            env,
            bindings_sql,
            bindings_blob,
            bindings_kv,
            bindings_functions,
        );

        let instance = template.instantiate().await.map_err(|err| match err {
            FunctionInstanceInitError::MissingExport => DeploymentInitError::MissingExport,
        })?;

        Ok(Self {
            function_id,
            template,
            instance: Rc::new(instance),
        })
    }

    pub(crate) async fn handle_request(&mut self, header: FetchRequestHeader, body: Option<FetchRequestBody>) -> Pin<Box<dyn Future<Output = Result<SerializedFunctionResource<FunctionResponse>, FunctionDeploymentHandleRequestError>>>> {
        let instance = self.instance.clone();

        let instance = if *instance.has_panicked.borrow() {
            let instance = self.template.instantiate().await.unwrap();
            let instance = Rc::new(instance);
            self.instance = instance.clone();
            instance
        } else {
            instance
        };

        Box::pin(async move {
            let mut header = header;
            let resource = {
                let mut data = instance.store.lock().await;
                let data = data.data_mut();
                if let Some(body) = body {
                    header.body_resource_id = Some(data.resource_add(Resource::RequestBody(body)));
                }
                data.resource_add(Resource::FetchRequest(SerializableResource::Raw(header)))
            };

            let result = FunctionFuture::new(instance.clone(), instance.invoke_http_trigger(&resource).await).await;

            {
                let mut function_state = instance.store.lock().await;
                let function_state = function_state.data_mut();
                for background_task in function_state.tasks_background.drain(..) {
                    tokio::task::spawn_local(FunctionUnitFuture::new(instance.clone(), background_task));
                }
            }

            result
                .map(|response_resource| SerializedFunctionResource::new(instance, response_resource))
                .map_err(|err| match err {
                    FunctionFutureError::FunctionPanicked => FunctionDeploymentHandleRequestError::FunctionPanicked,
                })
        })
    }
}

#[derive(Debug, Error)]
pub(crate) enum DeploymentInitError {
    #[error("function requested import that fx runtime does not provide")]
    MissingImport,
    #[error("function does not provide export that fx runtime expects")]
    MissingExport,
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

#[derive(Hash, Eq, PartialEq, Clone, Serialize, Deserialize, Debug)]
pub struct FunctionId {
    id: String,
}

impl FunctionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
        }
    }

    pub fn as_string(&self) -> String {
        self.id.clone()
    }

    pub fn as_str(&self) -> &str {
        self.id.as_str()
    }
}

impl Into<String> for FunctionId {
    fn into(self) -> String {
        self.id
    }
}

impl Into<String> for &FunctionId {
    fn into(self) -> String {
        self.id.clone()
    }
}

struct FunctionTemplate {
    wasmtime: Rc<wasmtime::Engine>,
    limit_memory_bytes: Option<usize>,
    local_worker: LocalWorkerController,
    logger_tx: flume::Sender<LogMessageEvent>,
    sql_tx: flume::Sender<SqlMessage>,
    kv_tx: flume::Sender<KvMessage>,
    blob_tx: flume::Sender<BlobMessage>,
    function_id: FunctionId,
    instance_template: wasmtime::InstancePre<FunctionInstanceState>,
    env: HashMap<String, String>,
    bindings_sql: HashMap<String, SqlBindingConfig>,
    bindings_blob: HashMap<String, BlobBindingConfig>,
    bindings_kv: HashMap<String, KvBindingConfig>,
    bindings_functions: HashMap<String, FunctionBindingConfig>,
}

impl FunctionTemplate {
    pub fn new(
        wasmtime: Rc<wasmtime::Engine>,
        limit_memory_bytes: Option<usize>,
        local_worker: LocalWorkerController,
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        kv_tx: flume::Sender<KvMessage>,
        blob_tx: flume::Sender<BlobMessage>,
        function_id: FunctionId,
        instance_template: wasmtime::InstancePre<FunctionInstanceState>,
        env: HashMap<String, String>,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
        bindings_kv: HashMap<String, KvBindingConfig>,
        bindings_functions: HashMap<String, FunctionBindingConfig>,
    ) -> Self {
        Self {
            wasmtime,
            limit_memory_bytes,
            local_worker,
            logger_tx,
            sql_tx,
            kv_tx,
            blob_tx,
            function_id,
            instance_template,
            env,
            bindings_sql,
            bindings_blob,
            bindings_kv,
            bindings_functions,
        }
    }

    pub async fn instantiate(&self) -> Result<FunctionInstance, FunctionInstanceInitError> {
        FunctionInstance::new(
            &self.wasmtime,
            self.limit_memory_bytes.clone(),
            self.local_worker.clone(),
            self.logger_tx.clone(),
            self.sql_tx.clone(),
            self.kv_tx.clone(),
            self.blob_tx.clone(),
            self.function_id.clone(),
            &self.instance_template,
            self.env.clone(),
            self.bindings_sql.clone(),
            self.bindings_blob.clone(),
            self.bindings_kv.clone(),
            self.bindings_functions.clone(),
        ).await
    }
}
