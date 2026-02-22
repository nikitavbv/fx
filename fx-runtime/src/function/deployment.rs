use thiserror::Error;

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
    module: wasmtime::Module,
    instance_template: wasmtime::InstancePre<FunctionInstanceState>,
    pub(crate) instance: Rc<FunctionInstance>,
}

impl FunctionDeployment {
    pub async fn new(
        wasmtime: &wasmtime::Engine,
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        function_id: FunctionId,
        module: wasmtime::Module,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
    ) -> Result<Self, DeploymentInitError> {
        let mut linker = wasmtime::Linker::<FunctionInstanceState>::new(wasmtime);

        linker.func_wrap("fx", "fx_log", fx_log_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_serialize", fx_resource_serialize_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_move_from_host", fx_resource_move_from_host_handler).unwrap();
        linker.func_wrap("fx", "fx_resource_drop", fx_resource_drop_handler).unwrap();
        linker.func_wrap("fx", "fx_sql_exec", fx_sql_exec_handler).unwrap();
        linker.func_wrap("fx", "fx_sql_migrate", fx_sql_migrate_handler).unwrap();
        linker.func_wrap("fx", "fx_future_poll", fx_future_poll_handler).unwrap();
        linker.func_wrap("fx", "fx_sleep", fx_sleep_handler).unwrap();
        linker.func_wrap("fx", "fx_random", fx_random_handler).unwrap();
        linker.func_wrap("fx", "fx_time", fx_time_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_put", fx_blob_put_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_get", fx_blob_get_handler).unwrap();
        linker.func_wrap("fx", "fx_blob_delete", fx_blob_delete_handler).unwrap();
        linker.func_wrap("fx", "fx_fetch", fx_fetch_handler).unwrap();
        linker.func_wrap("fx", "fx_metrics_counter_register", fx_metrics_counter_register_handler).unwrap();
        linker.func_wrap("fx", "fx_metrics_counter_increment", fx_metrics_counter_increment_handler).unwrap();
        linker.func_wrap("fx", "fx_stream_frame_read", fx_stream_frame_read_handler).unwrap();

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

        let instance = FunctionInstance::new(wasmtime, logger_tx, sql_tx, function_id.clone(), &instance_template, bindings_sql, bindings_blob).await
            .map_err(|err| match err {
                FunctionInstanceInitError::MissingExport => DeploymentInitError::MissingExport,
            })?;

        Ok(Self {
            function_id,
            module,
            instance_template,
            instance: Rc::new(instance),
        })
    }

    pub(crate) fn handle_request(&self, header: FetchRequestHeader, body: FetchRequestBody) -> Pin<Box<dyn Future<Output = Result<SerializedFunctionResource<FunctionResponse>, FunctionDeploymentHandleRequestError>>>> {
        let instance = self.instance.clone();

        Box::pin(async move {
            let mut header = header;
            let resource = {
                let mut data = instance.store.lock().await;
                let data = data.data_mut();
                header.body_resource_id = Some(data.resource_add(Resource::RequestBody(body)));
                data.resource_add(Resource::FetchRequest(SerializableResource::Raw(header)))
            };

            FunctionFuture::new(instance.clone(), instance.invoke_http_trigger(&resource).await).await
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
