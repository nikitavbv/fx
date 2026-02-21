use {
    std::{rc::Rc, collections::HashMap, pin::Pin, task::Poll},
    futures::FutureExt,
    futures_intrusive::sync::LocalMutex,
    thiserror::Error,
    wasmtime::{AsContextMut, AsContext},
    slotmap::SlotMap,
    serde::{Serialize, Deserialize},
    crate::v2::{
        logs::LogMessageEvent,
        SqlMessage,
        SqlBindingConfig,
        BlobBindingConfig,
        fx_log_handler,
        fx_resource_serialize_handler,
        fx_resource_move_from_host_handler,
        fx_resource_drop_handler,
        fx_sql_exec_handler,
        fx_sql_migrate_handler,
        fx_future_poll_handler,
        fx_sleep_handler,
        fx_random_handler,
        fx_time_handler,
        fx_blob_get_handler,
        fx_blob_put_handler,
        fx_blob_delete_handler,
        fx_fetch_handler,
        fx_metrics_counter_increment_handler,
        fx_metrics_counter_register_handler,
        fx_stream_frame_read_handler,
        FetchRequestHeader,
        SerializedFunctionResource,
        FunctionDeploymentHandleRequestError,
        FunctionResponse,
        FetchRequestBody,
        Resource,
        SerializableResource,
        FunctionFuture,
        FunctionFutureError,
        FunctionFuturePollError,
        FunctionResourceId,
        FuturePollResult,
        ResourceId,
        FunctionMetricsState,
        FutureResource,
        FetchRequestBodyInner,
        serialize_request_body_full,
        serialize_partially_read_stream,
    },
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

pub(crate) struct FunctionInstance {
    instance: wasmtime::Instance,
    pub(crate) store: LocalMutex<wasmtime::Store<FunctionInstanceState>>,
    memory: wasmtime::Memory,
    // fx apis:
    fn_future_poll: wasmtime::TypedFunc<u64, i64>,
    fn_resource_serialize: wasmtime::TypedFunc<u64, u64>,
    fn_resource_serialized_ptr: wasmtime::TypedFunc<u64, i64>,
    fn_resource_drop: wasmtime::TypedFunc<u64, ()>,
    // triggers:
    fn_trigger_http: wasmtime::TypedFunc<u64, u64>,
}

impl FunctionInstance {
    pub async fn new(
        wasmtime: &wasmtime::Engine,
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        function_id: FunctionId,
        instance_template: &wasmtime::InstancePre<FunctionInstanceState>,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
    ) -> Result<Self, FunctionInstanceInitError> {
        let mut store = wasmtime::Store::new(wasmtime, FunctionInstanceState::new(logger_tx, sql_tx, function_id, bindings_sql, bindings_blob));
        let instance = instance_template.instantiate_async(&mut store).await.unwrap();

        let memory = instance.get_memory(store.as_context_mut(), "memory").unwrap();

        let fn_future_poll = instance.get_typed_func::<u64, i64>(store.as_context_mut(), "_fx_future_poll")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_resource_serialize = instance.get_typed_func::<u64, u64>(store.as_context_mut(), "_fx_resource_serialize")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_resource_serialized_ptr = instance.get_typed_func::<u64, i64>(store.as_context_mut(), "_fx_resource_serialized_ptr")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_resource_drop = instance.get_typed_func(store.as_context_mut(), "_fx_resource_drop")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;

        let fn_trigger_http = instance.get_typed_func(store.as_context_mut(), "__fx_handler_http")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;

        // We are using async calls to exported functions to enable epoch-based preemption.
        // We also allow functions to handle concurrent requests. That introduces an interesting
        // edge case: once preempted, function has to resume execution for the same future and
        // request that triggered it. You cannot just resume execution with a different function call.
        // That means that while we use call_async, we need somehow to guarantee that each function
        // call will be executed to completion before fx function does anything else.
        // Using tokio::sync::Mutex would go against the idea of having no sync between threads and atomics,
        // so given this is a single-threaded runtime, we can use LocalMutex instead.
        let store = LocalMutex::new(store, false);

        Ok(Self {
            instance,
            store,
            memory,
            fn_future_poll,
            fn_resource_serialize,
            fn_resource_serialized_ptr,
            fn_resource_drop,
            fn_trigger_http,
        })
    }

    pub(crate) async fn future_poll(&self, future_id: &FunctionResourceId, waker: std::task::Waker) -> Result<Poll<()>, FunctionFuturePollError> {
        let mut store = self.store.lock().await;
        store.data_mut().waker = Some(waker);
        let future_poll_result = self.fn_future_poll.call_async(store.as_context_mut(), future_id.as_u64()).await;
        drop(store);

        let future_poll_result = future_poll_result.map_err(|err| {
            let trap = err.downcast::<wasmtime::Trap>().unwrap();
            match trap {
                wasmtime::Trap::UnreachableCodeReached => FunctionFuturePollError::FunctionPanicked,
                other => panic!("unexpected trap: {other:?}"),
            }
        })?;

        Ok(match FuturePollResult::try_from(future_poll_result).unwrap() {
            FuturePollResult::Pending => Poll::Pending,
            FuturePollResult::Ready => Poll::Ready(()),
        })
    }

    async fn resource_serialize(&self, resource_id: &FunctionResourceId) -> u64 {
        let mut store = self.store.lock().await;
        self.fn_resource_serialize.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap() as u64
    }

    async fn resource_serialized_ptr(&self, resource_id: &FunctionResourceId) -> u64 {
        let mut store = self.store.lock().await;
        self.fn_resource_serialized_ptr.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap() as u64
    }

    pub(crate) async fn resource_drop(&self, resource_id: &FunctionResourceId) {
        let mut store = self.store.lock().await;
        self.fn_resource_drop.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap();
    }

    pub(crate) async fn move_serializable_resource_to_host(&self, resource_id: &FunctionResourceId) -> Vec<u8> {
        let len = self.resource_serialize(resource_id).await as usize;
        let ptr = self.resource_serialized_ptr(resource_id).await as usize;

        let resource_data = {
            let store = self.store.lock().await;
            let view = self.memory.data(store.as_context());
            view[ptr..ptr+len].to_owned()
        };

        self.resource_drop(resource_id).await;

        resource_data
    }

    async fn invoke_http_trigger(&self, resource_id: &ResourceId) -> FunctionResourceId {
        let store = self.store.lock();
        FunctionResourceId::new(self.fn_trigger_http.call_async(store.await.as_context_mut(), resource_id.as_u64()).await.unwrap() as u64)
    }
}

#[derive(Debug, Error)]
enum FunctionInstanceInitError {
    #[error("function does not provide export that fx runtime expects to be present")]
    MissingExport,
}

pub(crate) struct FunctionInstanceState {
    waker: Option<std::task::Waker>,
    pub(crate) logger_tx: flume::Sender<LogMessageEvent>,
    pub(crate) sql_tx: flume::Sender<SqlMessage>,
    pub(crate) function_id: FunctionId,
    resources: SlotMap<slotmap::DefaultKey, Resource>,
    pub(crate) bindings_sql: HashMap<String, SqlBindingConfig>,
    pub(crate) bindings_blob: HashMap<String, BlobBindingConfig>,
    pub(crate) http_client: reqwest::Client,
    pub(crate) metrics: FunctionMetricsState,
}

impl FunctionInstanceState {
    pub fn new(
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        function_id: FunctionId,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
    ) -> Self {
        Self {
            waker: None,
            logger_tx,
            sql_tx,
            function_id,
            resources: SlotMap::new(),
            bindings_sql,
            bindings_blob,
            http_client: reqwest::Client::new(),
            metrics: FunctionMetricsState::new(),
        }
    }

    pub fn resource_add(&mut self, resource: Resource) -> ResourceId {
        ResourceId::from(self.resources.insert(resource))
    }

    pub fn resource_serialize(&mut self, resource_id: &ResourceId) -> usize {
        let resource = self.resources.detach(resource_id.into()).unwrap();
        let (resource, serialized_size) = match resource {
            Resource::FetchRequest(req) => {
                let serialized = req.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::FetchRequest(serialized), serialized_size)
            },
            Resource::SqlQueryResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::SqlQueryResult(FutureResource::Ready(serialized)), serialized_size)
            },
            Resource::SqlMigrationResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::SqlMigrationResult(FutureResource::Ready(serialized)), serialized_size)
            }
            Resource::UnitFuture(_) => panic!("unit future cannot be serialized"),
            Resource::BlobGetResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::BlobGetResult(FutureResource::Ready(serialized)), serialized_size)
            },
            Resource::FetchResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::FetchResult(FutureResource::Ready(serialized)), serialized_size)
            },
            Resource::RequestBody(v) => match v.0 {
                FetchRequestBodyInner::Full(v) => {
                    let serialized = serialize_request_body_full(v);
                    let serialized_size = serialized.len();
                    (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::FullSerialized(serialized))), serialized_size)
                },
                FetchRequestBodyInner::FullSerialized(serialized) => {
                    let serialized_size = serialized.len();
                    (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::FullSerialized(serialized))), serialized_size)
                },
                FetchRequestBodyInner::Stream(_) => panic!("resource is not yet ready for serialization"),
                FetchRequestBodyInner::PartiallyReadStream { stream, frame } => {
                    let frame_serialized = serialize_partially_read_stream(frame);
                    let serialized_size = frame_serialized.len();
                    (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStreamSerialized { frame_serialized, stream })), serialized_size)
                },
                FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized } => {
                    let serialized_size = frame_serialized.len();
                    (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStreamSerialized { frame_serialized, stream })), serialized_size)
                }
            },
        };
        self.resources.reattach(resource_id.into(), resource);
        serialized_size
    }

    pub fn resource_poll(&mut self, resource_id: &ResourceId) -> Poll<()> {
        let resource = self.resources.detach(resource_id.into()).unwrap();

        let mut cx = std::task::Context::from_waker(self.waker.as_ref().unwrap());
        let (resource, poll_result) = match resource {
            Resource::FetchRequest(v) => (Resource::FetchRequest(v), Poll::Ready(())),
            Resource::SqlQueryResult(v) => {
                let (resource, poll_result) = match v {
                    FutureResource::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                    FutureResource::Future(mut future) => {
                        let poll_result = future.poll_unpin(&mut cx);
                        match poll_result {
                            Poll::Pending => (FutureResource::Future(future), Poll::Pending),
                            Poll::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                        }
                    }
                };
                (Resource::SqlQueryResult(resource), poll_result)
            },
            Resource::SqlMigrationResult(v) => {
                let (resource, poll_result) = match v {
                    FutureResource::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                    FutureResource::Future(mut future) => {
                        let poll_result = future.poll_unpin(&mut cx);
                        match poll_result {
                            Poll::Pending => (FutureResource::Future(future), Poll::Pending),
                            Poll::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                        }
                    }
                };
                (Resource::SqlMigrationResult(resource), poll_result)
            }
            Resource::UnitFuture(mut v) => {
                let poll_result = v.poll_unpin(&mut cx);
                (Resource::UnitFuture(v), poll_result)
            },
            Resource::BlobGetResult(v) => {
                let (resource, poll_result) = match v {
                    FutureResource::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                    FutureResource::Future(mut future) => {
                        let poll_result = future.poll_unpin(&mut cx);
                        match poll_result {
                            Poll::Pending => (FutureResource::Future(future), Poll::Pending),
                            Poll::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                        }
                    }
                };
                (Resource::BlobGetResult(resource), poll_result)
            },
            Resource::FetchResult(v) => {
                let (resource, poll_result) = match v {
                    FutureResource::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                    FutureResource::Future(mut future) => {
                        let poll_result = future.poll_unpin(&mut cx);
                        match poll_result {
                            Poll::Pending => (FutureResource::Future(future), Poll::Pending),
                            Poll::Ready(v) => (FutureResource::Ready(v), Poll::Ready(())),
                        }
                    }
                };
                (Resource::FetchResult(resource), poll_result)
            },
            Resource::RequestBody(v) => match v.0 {
                FetchRequestBodyInner::Full(v) => (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::Full(v))), Poll::Ready(())),
                FetchRequestBodyInner::FullSerialized(v) => (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::FullSerialized(v))), Poll::Ready(())),
                FetchRequestBodyInner::Stream(mut stream) => {
                    use hyper::body::Body;

                    let poll_result = stream.as_mut().poll_frame(&mut cx);

                    match poll_result {
                        Poll::Pending => (Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::Stream(stream))), Poll::Pending),
                        Poll::Ready(frame) => (
                            Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStream {
                                frame,
                                stream,
                            })),
                            Poll::Ready(()),
                        ),
                    }
                },
                FetchRequestBodyInner::PartiallyReadStream { stream, frame } => (
                    Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStream { stream, frame })),
                    Poll::Ready(()),
                ),
                FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized } => (
                    Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized })),
                    Poll::Ready(())
                ),
            },
        };

        self.resources.reattach(resource_id.into(), resource);

        poll_result
    }

    pub fn resource_remove(&mut self, resource_id: &ResourceId) -> Resource {
        self.resources.remove(resource_id.into()).unwrap()
    }

    pub(crate) fn stream_read_frame(&mut self, resource_id: &ResourceId) -> Vec<u8> {
        let resource = self.resources.detach(resource_id.into()).unwrap();

        let (resource, serialized_frame) = match resource {
            Resource::BlobGetResult(_)
            | Resource::FetchRequest(_)
            | Resource::FetchResult(_)
            | Resource::SqlQueryResult(_)
            | Resource::SqlMigrationResult(_)
            | Resource::UnitFuture(_) => panic!("resource of this type does not support reading frames"),
            Resource::RequestBody(v) => match v.0 {
                FetchRequestBodyInner::Full(_)
                | FetchRequestBodyInner::Stream(_)
                | FetchRequestBodyInner::PartiallyReadStream { .. } => panic!("request body has to be serialized first"),
                FetchRequestBodyInner::FullSerialized(v) => (None, v),
                FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized } => (Some(Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::Stream(stream)))), frame_serialized),
            },
        };

        if let Some(resource) = resource {
            self.resources.reattach(resource_id.into(), resource);
        } else {
            self.resources.remove(resource_id.into());
        }

        serialized_frame
    }
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
