use {
    std::{collections::HashMap, task::Poll, cell::RefCell, rc::Rc, time::Duration},
    tracing::error,
    thiserror::Error,
    futures_intrusive::sync::LocalMutex,
    futures::{FutureExt, future::LocalBoxFuture},
    wasmtime::{AsContextMut, AsContext},
    fx_types::{capnp, abi_http_capnp},
    crate::{
        function::{abi::FuturePollResult, resource::FunctionStreamResourceId},
        effects::{
            logs::LogMessageEvent,
            metrics::FunctionMetricsState,
        },
        tasks::{sql::SqlController, worker::LocalWorkerController, kv::KvMessage, blob::BlobMessage},
        definitions::bindings::{SqlBindingConfig, BlobBindingConfig, FunctionBindingConfig, KvBindingConfig},
        resources::{
            FunctionResourceId,
            FunctionResources,
            resource::FetchRequestHeaderResourceKey,
        },
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
        sql_controller: SqlController,
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
            sql_controller,
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
            FuturePollResult::NotFound => todo!(),
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

    pub(crate) async fn stream_frame_poll(&self, resource_id: &FunctionStreamResourceId, waker: std::task::Waker) -> Poll<()> {
        let mut store = self.store.lock().await;
        store.data_mut().waker = Some(waker);
        let frame_poll_result = self.fn_stream_frame_poll.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap();
        drop(store);

        match FuturePollResult::try_from(frame_poll_result).unwrap() {
            FuturePollResult::Pending => Poll::Pending,
            FuturePollResult::Ready => Poll::Ready(()),
            FuturePollResult::NotFound => todo!(),
        }
    }

    pub(crate) async fn stream_advance(&self, resource_id: &FunctionStreamResourceId) {
        let mut store = self.store.lock().await;
        self.fn_stream_advance.call_async(store.as_context_mut(), resource_id.as_u64()).await.unwrap();
    }
}

pub(crate) mod stream_frame_read_v2 {
    use super::*;

    #[derive(Debug, Error)]
    pub(crate) enum StreamFrameReadError {
        #[error("function panicked while reading next frame")]
        FunctionPanicked,
    }

    impl FunctionInstance {
        pub(crate) async fn stream_frame_read_v2(&self, resource_id: &FunctionStreamResourceId) -> Result<Option<Vec<u8>>, StreamFrameReadError> {
            let len = {
                let mut store = self.store.lock().await;
                self.fn_stream_frame_serialize.call_async(store.as_context_mut(), resource_id.as_u64()).await
                    .map_err(|err| {
                        // TODO: forward backtraces to management thread (or logger thread)
                        let trap = err.downcast::<wasmtime::Trap>().unwrap();
                        match trap {
                            wasmtime::Trap::UnreachableCodeReached => StreamFrameReadError::FunctionPanicked,
                            other => panic!("unexpected trap: {other:?}"),
                        }
                    })? as usize
            };
            let ptr = self.resource_serialized_ptr(&FunctionResourceId::new(resource_id.as_u64())).await as usize; // TODO: remove this cast

            let frame_data = {
                let store = self.store.lock().await;
                let view = self.memory.data(store.as_context());
                view[ptr..ptr+len].to_owned()
            };

            let reader = capnp::serialize::read_message_from_flat_slice(&mut frame_data.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
            let response = reader.get_root::<abi_http_capnp::http_body_frame::Reader>().unwrap();

            Ok(match response.get_frame().which().unwrap() {
                abi_http_capnp::http_body_frame::frame::Which::StreamEnd(_) => None,
                abi_http_capnp::http_body_frame::frame::Which::Bytes(v) => Some(v.unwrap().to_vec()),
            })
        }
    }
}

pub(crate) mod invoke_http_trigger {
    use super::*;

    #[derive(Debug, Error)]
    pub(crate) enum InvokeError {
        #[error("function is busy handling other requests and cannot accept new request")]
        FunctionBusy,
        #[error("function panicked when invoked")]
        FunctionPanicked,
    }

    impl FunctionInstance {
        pub(crate) async fn invoke_http_trigger(&self, resource_id: &FetchRequestHeaderResourceKey) -> Result<FunctionResourceId, InvokeError> {
            let mut store = tokio::select! {
                store = self.store.lock() => store,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    error!("invoke_http_trigger: timeout when acquiring store lock");
                    return Err(InvokeError::FunctionBusy);
                },
            };
            store.set_epoch_deadline(SCHEDULING_YIELD_INTERVALS);
            Ok(FunctionResourceId::new(
                self.fn_handler.call_async(store.as_context_mut(), resource_id.into()).await
                    .map_err(|err| {
                        // TODO: forward backtraces to management thread (or logger thread)
                        let trap = err.downcast::<wasmtime::Trap>().unwrap();
                        match trap {
                            wasmtime::Trap::UnreachableCodeReached => InvokeError::FunctionPanicked,
                            other => panic!("unexpected trap: {other:?}"),
                        }
                    })? as u64)
            )
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum FunctionInstanceInitError {
    #[error("function does not provide export that fx runtime expects to be present")]
    MissingExport,
}

pub(crate) struct FunctionInstanceState {
    limits: wasmtime::StoreLimits,

    pub(crate) waker: Option<std::task::Waker>,
    pub(crate) local_worker: LocalWorkerController,
    pub(crate) logger_tx: flume::Sender<LogMessageEvent>,
    pub(crate) sql_controller: SqlController,
    pub(crate) kv_tx: flume::Sender<KvMessage>,
    pub(crate) blob_tx: flume::Sender<BlobMessage>,
    pub(crate) function_id: FunctionId,

    pub(crate) resource_set: FunctionResources,
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
        sql_controller: SqlController,
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
            sql_controller,
            kv_tx,
            blob_tx,
            function_id,
            env,

            resource_set: FunctionResources::new(),
            tasks_background: Vec::new(),

            bindings_sql,
            bindings_blob,
            bindings_kv,
            bindings_functions,
            http_client: reqwest::Client::new(),
            metrics: FunctionMetricsState::new(),
        }
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
    resource_id: FunctionStreamResourceId,

    inner_poll_future: Option<LocalBoxFuture<'static, Poll<()>>>,
}

impl FunctionFramePollFuture {
    pub(crate) fn new(instance: Rc<FunctionInstance>, resource_id: FunctionStreamResourceId) -> Self {
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
