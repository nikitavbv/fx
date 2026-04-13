use {
    std::{collections::HashMap, task::Poll, cell::RefCell, rc::Rc},
    thiserror::Error,
    futures_intrusive::sync::LocalMutex,
    futures::{FutureExt, StreamExt, future::LocalBoxFuture},
    slotmap::SlotMap,
    wasmtime::{AsContextMut, AsContext},
    send_wrapper::SendWrapper,
    fx_types::{capnp, abi_http_capnp, abi_kv_capnp},
    crate::{
        function::abi::FuturePollResult,
        effects::{
            logs::LogMessageEvent,
            metrics::FunctionMetricsState,
            fetch::{FetchResult, FetchResultWithBodyResource, HttpStreamError},
            kv::KvSubscriptionResource,
        },
        tasks::{sql::SqlMessage, worker::LocalWorkerController, kv::KvMessage, blob::BlobMessage},
        definitions::bindings::{SqlBindingConfig, BlobBindingConfig, FunctionBindingConfig, KvBindingConfig},
        resources::{
            FunctionResourceId,
            ResourceId,
            Resource,
            future::FutureResource,
            serialize::{serialize_request_body_full, serialize_partially_read_stream, SerializableResource},
        },
        triggers::http::{FetchRequestBodyInner, FetchRequestBody, HttpBody, HttpBodyInner},
    },
    super::FunctionId,
};

const SCHEDULING_YIELD_INTERVALS: u64 = 10; // yield every 10ms

pub(crate) struct FunctionInstance {
    // lifecycle flags:
    pub(crate) has_panicked: RefCell<bool>,
    // wasm instance:
    pub(crate) store: LocalMutex<wasmtime::Store<FunctionInstanceState>>,
    memory: wasmtime::Memory,
    // fx apis:
    fn_future_poll: wasmtime::TypedFunc<u64, i64>,
    fn_resource_serialize: wasmtime::TypedFunc<u64, u64>,
    fn_resource_serialized_ptr: wasmtime::TypedFunc<u64, i64>,
    fn_resource_drop: wasmtime::TypedFunc<u64, ()>,
    fn_stream_frame_poll: wasmtime::TypedFunc<u64, i64>,
    fn_stream_frame_serialize: wasmtime::TypedFunc<u64, u64>,
    fn_stream_advance: wasmtime::TypedFunc<u64, ()>,
    // triggers:
    fn_handler: wasmtime::TypedFunc<u64, u64>,
}

impl FunctionInstance {
    pub async fn new(
        wasmtime: &wasmtime::Engine,
        limit_memory_bytes: Option<usize>,
        local_worker: LocalWorkerController,
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        kv_tx: flume::Sender<KvMessage>,
        blob_tx: flume::Sender<BlobMessage>,
        function_id: FunctionId,
        instance_template: &wasmtime::InstancePre<FunctionInstanceState>,
        env: HashMap<String, String>,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
        bindings_kv: HashMap<String, KvBindingConfig>,
        bindings_functions: HashMap<String, FunctionBindingConfig>,
    ) -> Result<Self, FunctionInstanceInitError> {
        let mut store = wasmtime::Store::new(wasmtime, FunctionInstanceState::new(
            limit_memory_bytes,
            local_worker,
            logger_tx,
            sql_tx,
            kv_tx,
            blob_tx,
            function_id,
            env,
            bindings_sql,
            bindings_blob,
            bindings_kv,
            bindings_functions
        ));
        store.limiter(|state| &mut state.limits);
        store.epoch_deadline_callback(|_store_ctx| {
            Ok(wasmtime::UpdateDeadline::YieldCustom(SCHEDULING_YIELD_INTERVALS, async {
                tokio::time::sleep(std::time::Duration::ZERO).await;
                tokio::task::yield_now().await;
            }.boxed()))
        });

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
        let fn_stream_frame_poll = instance.get_typed_func(store.as_context_mut(), "_fx_stream_frame_poll")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_stream_frame_serialize = instance.get_typed_func(store.as_context_mut(), "_fx_stream_frame_serialize")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_stream_advance = instance.get_typed_func(store.as_context_mut(), "_fx_stream_advance")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;

        let fn_handler = instance.get_typed_func(store.as_context_mut(), "__fx_handler")
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
            has_panicked: RefCell::new(false),
            store,
            memory,
            fn_future_poll,
            fn_resource_serialize,
            fn_resource_serialized_ptr,
            fn_resource_drop,
            fn_stream_frame_poll,
            fn_stream_frame_serialize,
            fn_stream_advance,
            fn_handler,
        })
    }

    pub(crate) async fn future_poll(&self, future_id: &FunctionResourceId, waker: std::task::Waker) -> Result<Poll<()>, FunctionFuturePollError> {
        let mut store = self.store.lock().await;
        store.data_mut().waker = Some(waker);
        let future_poll_result = self.fn_future_poll.call_async(store.as_context_mut(), future_id.as_u64()).await;
        drop(store);

        let future_poll_result = future_poll_result.map_err(|err| {
            // TODO: forward backtraces to management thread (or logger thread)
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

    pub(crate) async fn copy_serializable_resource_to_host(&self, resource_id: &FunctionResourceId) -> Vec<u8> {
        let len = self.resource_serialize(resource_id).await as usize;
        let ptr = self.resource_serialized_ptr(resource_id).await as usize;

        let store = self.store.lock().await;
        let view = self.memory.data(store.as_context());
        view[ptr..ptr+len].to_owned()
    }

    pub(crate) async fn move_serializable_resource_to_host(&self, resource_id: &FunctionResourceId) -> Vec<u8> {
        let resource_data = self.copy_serializable_resource_to_host(resource_id).await;
        self.resource_drop(resource_id).await;
        resource_data
    }

    pub(crate) async fn stream_frame_poll(&self, resource_id: &FunctionResourceId, waker: std::task::Waker) -> Poll<()> {
        let mut store = self.store.lock().await;
        store.data_mut().waker = Some(waker);
        let frame_poll_result = self.fn_stream_frame_poll.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap();
        drop(store);

        match FuturePollResult::try_from(frame_poll_result).unwrap() {
            FuturePollResult::Pending => Poll::Pending,
            FuturePollResult::Ready => Poll::Ready(()),
        }
    }

    pub(crate) async fn stream_frame_read_v2(&self, resource_id: &FunctionResourceId) -> Option<Vec<u8>> {
        let len = {
            let mut store = self.store.lock().await;
            self.fn_stream_frame_serialize.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap() as usize
        };
        let ptr = self.resource_serialized_ptr(resource_id).await as usize;

        let frame_data = {
            let store = self.store.lock().await;
            let view = self.memory.data(store.as_context());
            view[ptr..ptr+len].to_owned()
        };

        let reader = capnp::serialize::read_message_from_flat_slice(&mut frame_data.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
        let response = reader.get_root::<abi_http_capnp::http_body_frame::Reader>().unwrap();

        match response.get_frame().which().unwrap() {
            abi_http_capnp::http_body_frame::frame::Which::StreamEnd(_) => None,
            abi_http_capnp::http_body_frame::frame::Which::Bytes(v) => Some(v.unwrap().to_vec()),
        }
    }

    pub(crate) async fn stream_advance(&self, resource_id: &FunctionResourceId) {
        let mut store = self.store.lock().await;
        self.fn_stream_advance.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap();
    }

    pub(crate) async fn invoke_http_trigger(&self, resource_id: &ResourceId) -> FunctionResourceId {
        let mut store = self.store.lock().await;
        store.set_epoch_deadline(SCHEDULING_YIELD_INTERVALS);
        FunctionResourceId::new(self.fn_handler.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap() as u64)
    }
}

#[derive(Debug, Error)]
pub(crate) enum FunctionInstanceInitError {
    #[error("function does not provide export that fx runtime expects to be present")]
    MissingExport,
}

pub(crate) struct FunctionInstanceState {
    limits: wasmtime::StoreLimits,

    waker: Option<std::task::Waker>,
    pub(crate) local_worker: LocalWorkerController,
    pub(crate) logger_tx: flume::Sender<LogMessageEvent>,
    pub(crate) sql_tx: flume::Sender<SqlMessage>,
    pub(crate) kv_tx: flume::Sender<KvMessage>,
    pub(crate) blob_tx: flume::Sender<BlobMessage>,
    pub(crate) function_id: FunctionId,

    resources: SlotMap<slotmap::DefaultKey, Resource>,
    pub(crate) tasks_background: Vec<FunctionResourceId>,

    pub(crate) env: HashMap<String, String>,
    pub(crate) bindings_sql: HashMap<String, SqlBindingConfig>,
    pub(crate) bindings_blob: HashMap<String, BlobBindingConfig>,
    pub(crate) bindings_kv: HashMap<String, KvBindingConfig>,
    pub(crate) bindings_functions: HashMap<String, FunctionBindingConfig>, // host is key
    pub(crate) http_client: reqwest::Client,
    pub(crate) metrics: FunctionMetricsState,
}

impl FunctionInstanceState {
    pub fn new(
        limit_memory_bytes: Option<usize>,
        local_worker: LocalWorkerController,
        logger_tx: flume::Sender<LogMessageEvent>,
        sql_tx: flume::Sender<SqlMessage>,
        kv_tx: flume::Sender<KvMessage>,
        blob_tx: flume::Sender<BlobMessage>,
        function_id: FunctionId,
        env: HashMap<String, String>,
        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
        bindings_kv: HashMap<String, KvBindingConfig>,
        bindings_functions: HashMap<String, FunctionBindingConfig>,
    ) -> Self {
        let limits = wasmtime::StoreLimitsBuilder::new();

        let limits = match limit_memory_bytes {
            Some(limit_bytes) => limits.memory_size(limit_bytes).memories(1),
            None => limits,
        };

        Self {
            limits: limits.build(),

            waker: None,
            local_worker,
            logger_tx,
            sql_tx,
            kv_tx,
            blob_tx,
            function_id,
            env,

            resources: SlotMap::new(),
            tasks_background: Vec::new(),

            bindings_sql,
            bindings_blob,
            bindings_kv,
            bindings_functions,
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
            },
            Resource::SqlBatchResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::SqlBatchResult(FutureResource::Ready(serialized)), serialized_size)
            },
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

                match resource {
                    FetchResult::Inline(resource) => {
                        let (parts, body) = resource.into_parts();
                        let body = self.resource_add(Resource::HttpBody(body));
                        let serialized = SerializableResource::Raw(FetchResultWithBodyResource::new(parts, body)).map_to_serialized();
                        let serialized_size = serialized.serialized_size();
                        (Resource::FetchResult(FutureResource::Ready(FetchResult::BodyResource(serialized))), serialized_size)
                    },
                    FetchResult::BodyResource(resource) => {
                        let serialized_size = resource.serialized_size();
                        (Resource::FetchResult(FutureResource::Ready(FetchResult::BodyResource(resource))), serialized_size)
                    }
                }
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
                },
            },
            Resource::HttpBody(v) => match v.0 {
                HttpBodyInner::Empty => {
                    let frame_serialized = {
                        let mut message = capnp::message::Builder::new_default();
                        let serialized_frame = message.init_root::<abi_http_capnp::http_body_frame::Builder>();
                        let mut serialized_frame = serialized_frame.init_frame();

                        serialized_frame.set_stream_end(());

                        capnp::serialize::write_message_to_words(&message)
                    };
                    let serialized_size = frame_serialized.len();

                    (Resource::HttpBody(HttpBody(HttpBodyInner::FrameSerialized(frame_serialized))), serialized_size)
                },
                HttpBodyInner::Full(_) => todo!(),
                HttpBodyInner::FunctionStream(_) => todo!(), // note that we are trying to access different function resource here, so copying will be needed
                HttpBodyInner::Stream { stream, mut frame } => {
                    let frame = std::mem::take(&mut frame).unwrap().map_to_serialized();
                    let serialized_size = frame.serialized_size();

                    (Resource::HttpBody(HttpBody(HttpBodyInner::Stream {
                        stream,
                        frame: Some(frame),
                    })), serialized_size)
                },
                HttpBodyInner::StreamLocal(_) => todo!(),
                HttpBodyInner::StreamLocalPartiallyRead { stream, frame } => {
                    let frame = frame.map_to_serialized();
                    let serialized_size = frame.serialized_size();

                    (Resource::HttpBody(HttpBody(HttpBodyInner::StreamLocalPartiallyRead {
                        stream,
                        frame,
                    })), serialized_size)
                },
                HttpBodyInner::FrameSerialized(v) => {
                    let serialized_len = v.len();
                    (Resource::HttpBody(HttpBody(HttpBodyInner::FrameSerialized(v))), serialized_len)
                },
            },
            Resource::KvSetResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::KvSetResult(FutureResource::Ready(serialized)), serialized_size)
            },
            Resource::KvGetResult(v) => {
                let resource = match v {
                    FutureResource::Future(_) => panic!("resource is not yet ready for serialization"),
                    FutureResource::Ready(v) => v,
                };

                let serialized = resource.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (Resource::KvGetResult(FutureResource::Ready(serialized)), serialized_size)
            },
            Resource::KvSubscription(v) => match v {
                KvSubscriptionResource::Init(_) |
                KvSubscriptionResource::Stream(_) => panic!("resource is not yet for serialization"),
                KvSubscriptionResource::NextReady { stream, frame } => {
                    let frame_serialized = {
                        let mut message = capnp::message::Builder::new_default();
                        let serialized_frame = message.init_root::<abi_kv_capnp::kv_subscription_frame::Builder>();
                        let mut serialized_frame = serialized_frame.init_frame();

                        serialized_frame.set_bytes(&frame);

                        capnp::serialize::write_message_to_words(&message)
                    };
                    let serialized_size = frame_serialized.len();

                    (Resource::KvSubscription(KvSubscriptionResource::NextSerialized { stream, frame_serialized }), serialized_size)
                },
                KvSubscriptionResource::NextSerialized { stream, frame_serialized } => {
                    let serialized_size = frame_serialized.len();
                    (Resource::KvSubscription(KvSubscriptionResource::NextSerialized { stream, frame_serialized }), serialized_size)
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
            Resource::SqlQueryResult(mut v) => {
                let poll_result = v.poll(&mut cx);
                (Resource::SqlQueryResult(v), poll_result)
            },
            Resource::SqlMigrationResult(mut v) => {
                let poll_result = v.poll(&mut cx);
                (Resource::SqlMigrationResult(v), poll_result)
            },
            Resource::SqlBatchResult(mut v) => {
                let poll_result = v.poll(&mut cx);
                (Resource::SqlBatchResult(v), poll_result)
            },
            Resource::UnitFuture(mut v) => {
                let poll_result = v.poll_unpin(&mut cx);
                (Resource::UnitFuture(v), poll_result)
            },
            Resource::BlobGetResult(mut v) => {
                let poll_result = v.poll(&mut cx);
                (Resource::BlobGetResult(v), poll_result)
            },
            Resource::FetchResult(mut v) => {
                let poll_result = v.poll(&mut cx);
                (Resource::FetchResult(v), poll_result)
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
            Resource::HttpBody(v) => match v.0 {
                HttpBodyInner::Empty => todo!(),
                HttpBodyInner::Full(_) => todo!(),
                HttpBodyInner::Stream { mut stream, frame } => {
                    let poll_result = stream.poll_next_unpin(&mut cx);

                    match poll_result {
                        Poll::Pending => (Resource::HttpBody(HttpBody(HttpBodyInner::Stream { stream, frame: None })), Poll::Pending),
                        Poll::Ready(None) => (Resource::HttpBody(HttpBody(HttpBodyInner::Empty)), Poll::Ready(())),
                        Poll::Ready(Some(frame)) => (
                            Resource::HttpBody(HttpBody(HttpBodyInner::Stream {
                                stream,
                                frame: Some(SerializableResource::Raw(frame)),
                            })),
                            Poll::Ready(()),
                        ),
                    }
                },
                HttpBodyInner::FunctionStream(stream) => {
                    let mut stream = stream.replace(None).unwrap()
                        .map(|v| Result::<_, HttpStreamError>::Ok(hyper::body::Bytes::from(v)))
                        .boxed_local();

                    let poll_result = stream.poll_next_unpin(&mut cx);

                    let stream = SendWrapper::new(stream);

                    match poll_result {
                        Poll::Pending => (Resource::HttpBody(HttpBody(HttpBodyInner::StreamLocal(stream))), Poll::Pending),
                        Poll::Ready(None) => (Resource::HttpBody(HttpBody(HttpBodyInner::Empty)), Poll::Ready(())),
                        Poll::Ready(Some(frame)) => (
                            Resource::HttpBody(HttpBody(HttpBodyInner::StreamLocalPartiallyRead {
                                stream,
                                frame: SerializableResource::Raw(frame),
                            })),
                            Poll::Ready(()),
                        ),
                    }
                },
                HttpBodyInner::StreamLocal(mut stream) => {
                    let poll_result = stream.poll_next_unpin(&mut cx);

                    match poll_result {
                        Poll::Pending => (Resource::HttpBody(HttpBody(HttpBodyInner::StreamLocal(stream))), Poll::Pending),
                        Poll::Ready(None) => (Resource::HttpBody(HttpBody(HttpBodyInner::Empty)), Poll::Ready(())),
                        Poll::Ready(Some(frame)) => (
                            Resource::HttpBody(HttpBody(HttpBodyInner::StreamLocalPartiallyRead {
                                stream,
                                frame: SerializableResource::Raw(frame),
                            })),
                            Poll::Ready(()),
                        ),
                    }
                },
                HttpBodyInner::StreamLocalPartiallyRead { stream, frame } => todo!(),
                HttpBodyInner::FrameSerialized(v) => (Resource::HttpBody(HttpBody(HttpBodyInner::FrameSerialized(v))), Poll::Ready(())),
            },
            Resource::KvSetResult(mut v) => {
                let poll_result = v.poll(&mut cx);
                (Resource::KvSetResult(v), poll_result)
            },
            Resource::KvGetResult(mut v) => {
                let poll_result = v.poll(&mut cx);
                (Resource::KvGetResult(v), poll_result)
            },
            Resource::KvSubscription(v) => match v {
                KvSubscriptionResource::Init(mut v) => match v.poll_unpin(&mut cx) {
                    std::task::Poll::Pending => (Resource::KvSubscription(KvSubscriptionResource::Init(v)), Poll::Pending),
                    std::task::Poll::Ready(Ok(v)) => {
                        let mut stream = v.into_stream().boxed();

                        match stream.poll_next_unpin(&mut cx) {
                            std::task::Poll::Pending => (Resource::KvSubscription(KvSubscriptionResource::Stream(stream)), std::task::Poll::Pending),
                            std::task::Poll::Ready(None) => todo!(),
                            std::task::Poll::Ready(Some(frame)) => (Resource::KvSubscription(KvSubscriptionResource::NextReady {
                                stream,
                                frame,
                            }), std::task::Poll::Ready(())),
                        }
                    },
                    std::task::Poll::Ready(Err(err)) => todo!("handle error: {err:?}"),
                },
                KvSubscriptionResource::Stream(mut stream) => match stream.poll_next_unpin(&mut cx) {
                    std::task::Poll::Pending => (Resource::KvSubscription(KvSubscriptionResource::Stream(stream)), std::task::Poll::Pending),
                    std::task::Poll::Ready(None) => todo!(),
                    std::task::Poll::Ready(Some(frame)) => (
                        Resource::KvSubscription(KvSubscriptionResource::NextReady { stream, frame }),
                        std::task::Poll::Ready(()),
                    ),
                },
                KvSubscriptionResource::NextReady { stream, frame } => (Resource::KvSubscription(KvSubscriptionResource::NextReady { stream, frame }), std::task::Poll::Ready(())),
                KvSubscriptionResource::NextSerialized { stream, frame_serialized } => (Resource::KvSubscription(KvSubscriptionResource::NextSerialized { stream, frame_serialized }), std::task::Poll::Ready(())),
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
            | Resource::SqlBatchResult(_)
            | Resource::SqlMigrationResult(_)
            | Resource::UnitFuture(_)
            | Resource::KvSetResult(_)
            | Resource::KvGetResult(_) => panic!("resource of this type does not support reading frames"),
            Resource::RequestBody(v) => match v.0 {
                FetchRequestBodyInner::Full(_)
                | FetchRequestBodyInner::Stream(_)
                | FetchRequestBodyInner::PartiallyReadStream { .. } => panic!("request body has to be serialized first"),
                FetchRequestBodyInner::FullSerialized(v) => (None, v),
                FetchRequestBodyInner::PartiallyReadStreamSerialized { stream, frame_serialized } => (Some(Resource::RequestBody(FetchRequestBody(FetchRequestBodyInner::Stream(stream)))), frame_serialized),
            },
            Resource::HttpBody(v) => match v.0 {
                HttpBodyInner::Empty
                | HttpBodyInner::Full(_)
                | HttpBodyInner::Stream(_) => panic!("HttpBody has to be serialized first"),
                HttpBodyInner::FunctionStream(_) => todo!(),
                HttpBodyInner::StreamPartiallyRead { stream, frame } => (
                    Some(Resource::HttpBody(HttpBody(HttpBodyInner::Stream(stream)))),
                    frame.into_serialized()
                ),
                HttpBodyInner::StreamLocal(_) => todo!(),
                HttpBodyInner::StreamLocalPartiallyRead { stream, frame } => (
                    Some(Resource::HttpBody(HttpBody(HttpBodyInner::StreamLocal(stream)))),
                    frame.into_serialized()
                ),
                HttpBodyInner::FrameSerialized(v) => (Some(Resource::HttpBody(HttpBody(HttpBodyInner::Empty))), v),
            },
            Resource::KvSubscription(v) => match v {
                KvSubscriptionResource::Init(_)
                | KvSubscriptionResource::Stream(_)
                | KvSubscriptionResource::NextReady { .. } => panic!("KvSubscription has to be serialized first"),
                KvSubscriptionResource::NextSerialized { stream, frame_serialized } => (
                    Some(Resource::KvSubscription(KvSubscriptionResource::Stream(stream))),
                    frame_serialized,
                ),
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

/// Error that occured while polling function future
#[derive(Debug, Error)]
pub enum FunctionFuturePollError {
    /// Function panicked when future poll was callled
    #[error("function panicked")]
    FunctionPanicked,
}

pub(crate) struct FunctionFramePollFuture {
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,

    inner_poll_future: Option<LocalBoxFuture<'static, Poll<()>>>,
}

impl FunctionFramePollFuture {
    pub(crate) fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            instance,
            resource_id,
            inner_poll_future: None,
        }
    }
}

impl Future for FunctionFramePollFuture {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let inner_poll_future = std::mem::replace(&mut self.inner_poll_future, None);

        let mut inner_poll_future = match inner_poll_future {
            Some(v) => v,
            None => {
                let instance = self.instance.clone();
                let resource_id = self.resource_id.clone();
                async move {
                    let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
                    instance.stream_frame_poll(&resource_id, waker).await
                }.boxed_local()
            },
        };

        let result = match inner_poll_future.poll_unpin(cx) {
            Poll::Pending => {
                self.inner_poll_future = Some(inner_poll_future);
                Poll::Pending
            },
            Poll::Ready(Poll::Pending) => Poll::Pending,
            Poll::Ready(Poll::Ready(())) => Poll::Ready(()),
        };

        result
    }
}
