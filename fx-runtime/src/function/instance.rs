use {
    std::{collections::HashMap, task::Poll, cell::RefCell, rc::{Rc, Weak}, time::Duration},
    tracing::{error, warn},
    thiserror::Error,
    futures_intrusive::sync::LocalMutex,
    futures::{FutureExt, future::LocalBoxFuture},
    wasmtime::{AsContextMut, AsContext},
    zerocopy::FromBytes,
    send_wrapper::SendWrapper,
    fx_types::{abi::{FunctionHttpBodyFramePollResult, FunctionResponsePollResult}, capnp, abi_http_capnp},
    crate::{
        function::resource::FunctionStreamResourceId,
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
        triggers::http::HttpBody,
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
    fn_http_body_frame_poll: wasmtime::TypedFunc<u64, u64>,
    fn_bytes_drop: wasmtime::TypedFunc<u64, ()>,
    fn_background_task_poll: wasmtime::TypedFunc<u64, u64>,
    fn_function_response_poll: wasmtime::TypedFunc<u64, u64>,
    // triggers:
    fn_handler: wasmtime::TypedFunc<u64, u64>,
}

impl FunctionInstance {
    pub async fn new(
        wasmtime: &wasmtime::Engine,
        runtime_services: RuntimeServices,
        limit_memory_bytes: Option<usize>,
        function_id: FunctionId,
        instance_template: &wasmtime::InstancePre<FunctionInstanceState>,
        bindings: InstanceBindings,
    ) -> Result<Rc<Self>, FunctionInstanceInitError> {
        let mut store = wasmtime::Store::new(wasmtime, FunctionInstanceState::new(
            runtime_services,
            limit_memory_bytes,
            function_id,
            bindings,
        ));
        store.limiter(|state| &mut state.limits);
        store.epoch_deadline_callback(|_store_ctx| {
            Ok(wasmtime::UpdateDeadline::YieldCustom(SCHEDULING_YIELD_INTERVALS, async {
                tokio::time::sleep(std::time::Duration::ZERO).await;
                tokio::task::yield_now().await;
            }.boxed()))
        });

        let instance = instance_template.instantiate_async(&mut store).await
            .map_err(|err| {
                error!("failed to instantiate instance template: {err:?}");
                FunctionInstanceInitError::UnknownError
            })?;

        let memory = instance.get_memory(store.as_context_mut(), "memory")
            .ok_or(FunctionInstanceInitError::MissingMemory)?;

        let fn_http_body_frame_poll = instance.get_typed_func(store.as_context_mut(), "_fx_http_body_frame_poll")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_bytes_drop = instance.get_typed_func(store.as_context_mut(), "_fx_bytes_drop")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_background_task_poll = instance.get_typed_func(store.as_context_mut(), "_fx_background_task_poll")
            .map_err(|_| FunctionInstanceInitError::MissingExport)?;
        let fn_function_response_poll = instance.get_typed_func(store.as_context_mut(), "_fx_function_response_poll")
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

        let instance = Rc::new(Self {
            has_panicked: RefCell::new(false),
            store,
            memory,
            fn_http_body_frame_poll,
            fn_bytes_drop,
            fn_background_task_poll,
            fn_function_response_poll,
            fn_handler,
        });

        instance.store.lock().await.data_mut().self_instance = send_wrapper::SendWrapper::new(Rc::downgrade(&instance));

        Ok(instance)
    }

    pub(crate) async fn bytes_drop(&self, resource_id: &FunctionResourceId) -> Result<(), WasmFunctionCallError> {
        let mut store = self.store.lock().await;
        self.fn_bytes_drop.call_async(store.as_context_mut(), resource_id.as_u64()).await.map_err(WasmFunctionCallError::from)
    }
}

pub(crate) mod http_body_frame_poll {
    use super::*;

    #[derive(Debug, Error)]
    pub(crate) enum HttpBodyFramePollError {
        #[error("function abi returned incorrect data when polling http body stream")]
        AbiError,
        #[error("an assertion failed when polling http body stream")]
        AssertionError,
        #[error("function panicked when polling http body stream")]
        FunctionPanicked,
        #[error("function crashed when polling http body stream")]
        FunctionCrashed,
    }

    impl From<WasmFunctionCallError> for HttpBodyFramePollError {
        fn from(err: WasmFunctionCallError) -> Self {
            match err {
                WasmFunctionCallError::Panicked => Self::FunctionPanicked,
                WasmFunctionCallError::Crashed => Self::FunctionCrashed,
            }
        }
    }

    pub(crate) struct FunctionFrame {
        local_worker: LocalWorkerController,

        function_instance: SendWrapper<Rc<FunctionInstance>>,

        resource_id: FunctionResourceId,

        ptr: SendWrapper<*const u8>, // absolute pointer into guest linear memory
        len: usize,
    }

    impl Drop for FunctionFrame {
        fn drop(&mut self) {
            self.local_worker.bytes_drop((*self.function_instance).clone(), self.resource_id.clone());
        }
    }

    impl AsRef<[u8]> for FunctionFrame {
        fn as_ref(&self) -> &[u8] {
            // ptr points into instance's linear memory.
            // safe because wastime grows memory in place and base pointer cannot move.
            unsafe { std::slice::from_raw_parts(*self.ptr, self.len) }
        }
    }

    impl FunctionInstance {
        pub(crate) async fn http_body_frame_poll(&self, resource_id: &FunctionStreamResourceId, waker: std::task::Waker) -> Poll<Result<Option<FunctionFrame>, HttpBodyFramePollError>> {
            let mut store = self.store.lock().await;
            store.data_mut().waker = Some(waker);

            let async_operation_addr = self.fn_http_body_frame_poll.call_async(store.as_context_mut(), resource_id.as_u64()).await
                .map(|v| v as usize)
                .map_err(WasmFunctionCallError::from);
            let async_operation_addr = match async_operation_addr {
                Ok(v) => v,
                Err(err) => return Poll::Ready(Err(HttpBodyFramePollError::from(err))),
            };

            let poll_result = FunctionHttpBodyFramePollResult::read_from_bytes(
                &self.memory.data(store.as_context())[async_operation_addr..async_operation_addr+std::mem::size_of::<FunctionHttpBodyFramePollResult>()]
            ).map_err(|err| {
                warn!("failed to read FunctionHttpBodyFramePollResult: {err:?}");
                HttpBodyFramePollError::AbiError
            });

            let poll_result = match poll_result {
                Ok(v) => v,
                Err(err) => return Poll::Ready(Err(err)),
            };

            match poll_result.tag {
                2 => Poll::Pending,
                0 => Poll::Ready(Ok(None)),
                1 => Poll::Ready(Ok(Some({
                    let addr = poll_result.frame_bytes_addr as usize;
                    let len = poll_result.frame_bytes_len as usize;
                    let slice = &self.memory.data(store.as_context())[addr..addr+len];
                    FunctionFrame {
                        local_worker: store.data().runtime_services.local_worker.clone(),

                        function_instance: SendWrapper::new(
                            match store.data().self_instance.upgrade() {
                                Some(v) => v,
                                None => {
                                    error!("assertion error: expected upgrade() on self_instance to always succeed because that pointer is pointing to self");
                                    return Poll::Ready(Err(HttpBodyFramePollError::AssertionError));
                                },
                            }
                        ),

                        resource_id: poll_result.frame_bytes_resource_id.into(),

                        ptr: send_wrapper::SendWrapper::new(slice.as_ptr()),
                        len,
                    }
                }))),
                _other => Poll::Ready(Err(HttpBodyFramePollError::AbiError)),
            }
        }
    }
}
pub(crate) use http_body_frame_poll::FunctionFrame;

pub(crate) mod invoke_http_trigger {
    use super::*;

    #[derive(Debug, Error)]
    pub(crate) enum InvokeError {
        #[error("function is busy handling other requests and cannot accept new request")]
        Busy,
        #[error("function panicked when invoked")]
        Panicked,
        #[error("function stopped with unknown wasm trap when invoked")]
        Crashed,
    }

    impl From<WasmFunctionCallError> for InvokeError {
        fn from(err: WasmFunctionCallError) -> Self {
            match err {
                WasmFunctionCallError::Panicked => Self::Panicked,
                WasmFunctionCallError::Crashed => Self::Crashed,
            }
        }
    }

    impl FunctionInstance {
        pub(crate) async fn invoke_http_trigger(&self, resource_id: &FetchRequestHeaderResourceKey) -> Result<FunctionResourceId, InvokeError> {
            let mut store = tokio::select! {
                store = self.store.lock() => store,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    error!("invoke_http_trigger: timeout when acquiring store lock");
                    return Err(InvokeError::Busy);
                },
            };
            store.set_epoch_deadline(SCHEDULING_YIELD_INTERVALS);
            Ok(FunctionResourceId::new(
                self.fn_handler.call_async(store.as_context_mut(), resource_id.into()).await
                    .map_err(WasmFunctionCallError::from)
                    .map_err(InvokeError::from)? as u64)
            )
        }
    }
}

pub(crate) mod background_task_poll {
    use super::*;

    #[derive(Debug, Error, Eq, PartialEq)]
    pub(crate) enum BackgroundTaskPollError {
        #[error("function returned incorrect abi response when polling background task")]
        AbiError,
        #[error("function panicked when polling background task")]
        FunctionPanicked,
        #[error("function crashed when polling background task")]
        FunctionCrashed,
    }

    impl From<WasmFunctionCallError> for BackgroundTaskPollError {
        fn from(err: WasmFunctionCallError) -> Self {
            match err {
                WasmFunctionCallError::Panicked => Self::FunctionPanicked,
                WasmFunctionCallError::Crashed => Self::FunctionCrashed,
            }
        }
    }

    impl FunctionInstance {
        pub(crate) async fn background_task_poll(&self, resource_id: &FunctionResourceId, waker: std::task::Waker) -> Result<Poll<()>, BackgroundTaskPollError> {
            let mut store = self.store.lock().await;
            store.data_mut().waker = Some(waker);
            self.fn_background_task_poll.call_async(store.as_context_mut(), resource_id.as_u64()).await
                .map_err(WasmFunctionCallError::from)
                .map_err(BackgroundTaskPollError::from)
                .and_then(|v| match v {
                    1 => Ok(Poll::Pending),
                    0 => Ok(Poll::Ready(())),
                    _other => Err(BackgroundTaskPollError::AbiError),
                })
        }
    }
}

pub(crate) mod function_response_poll {
    use super::*;

    #[derive(Debug, Error, Eq, PartialEq)]
    pub(crate) enum FunctionResponsePollError {
        #[error("internal runtime assertion error")]
        AssertionError,

        #[error("function returned incorrect abi response when polling background task")]
        AbiError,
        #[error("function panicked when polling function response")]
        FunctionPanicked,
        #[error("function crashed when polling function response")]
        FunctionCrashed,

        #[error("failed to deserialize function response")]
        FailedToDeserialize,
        #[error("host resource referenced by function is not found")]
        HostResourceNotFound,
        #[error("function returned invalid status code")]
        InvalidStatusCode,
        #[error("function returned invalid headers")]
        InvalidHeaders,
    }

    impl From<WasmFunctionCallError> for FunctionResponsePollError {
        fn from(err: WasmFunctionCallError) -> Self {
            match err {
                WasmFunctionCallError::Panicked => Self::FunctionPanicked,
                WasmFunctionCallError::Crashed => Self::FunctionCrashed,
            }
        }
    }

    impl FunctionInstance {
        pub(crate) async fn function_response_poll(&self, resource_id: &FunctionResourceId, waker: std::task::Waker) -> Poll<Result<http::Response<HttpBody>, FunctionResponsePollError>> {
            let mut store = self.store.lock().await;
            store.data_mut().waker = Some(waker);

            let result = self.fn_function_response_poll.call_async(store.as_context_mut(), resource_id.as_u64()).await
                .map_err(WasmFunctionCallError::from)
                .map_err(FunctionResponsePollError::from);

            let async_operation_addr = match result {
                Ok(v) => v as usize,
                Err(err) => return Poll::Ready(Err(err)),
            };

            let poll_result = FunctionResponsePollResult::read_from_bytes(
                &self.memory.data(store.as_context())[async_operation_addr..async_operation_addr+std::mem::size_of::<FunctionResponsePollResult>()]
            ).map_err(|err| {
                warn!("failed to read AsyncResourcePollResult when polling function response: {err:?}");
                FunctionResponsePollError::AbiError
            });

            let result = match poll_result {
                Ok(v) => v,
                Err(err) => return Poll::Ready(Err(err)),
            };

            match result.tag {
                1 => Poll::Pending,
                0 => Poll::Ready({
                    let addr = result.response_bytes_addr as usize;
                    let len = result.response_bytes_len as usize;

                    let (body, status, headers) = {
                        let mut slice = &self.memory.data(store.as_context())[addr..addr+len];

                        let message_reader = capnp::serialize::read_message_from_flat_slice(&mut slice, capnp::message::ReaderOptions::default())
                            .map_err(|_| FunctionResponsePollError::FailedToDeserialize)?;
                        let response = message_reader.get_root::<abi_http_capnp::http_response::Reader>()
                            .map_err(|_| FunctionResponsePollError::FailedToDeserialize)?;

                        let body = response.get_body().which()
                            .map_err(|_| FunctionResponsePollError::FailedToDeserialize)?;

                        let status = ::http::StatusCode::from_u16(response.get_status())
                            .map_err(|_| FunctionResponsePollError::InvalidStatusCode)?;

                        let mut headers = Vec::new();
                        for header in response.get_headers().map_err(|_| FunctionResponsePollError::InvalidHeaders)? {
                            let name = header.get_name()
                                .map_err(|_| FunctionResponsePollError::InvalidHeaders)?;
                            let name = ::http::HeaderName::from_bytes(name.as_bytes())
                                .map_err(|_| FunctionResponsePollError::InvalidHeaders)?;

                            let value = header.get_value()
                                .map_err(|_| FunctionResponsePollError::InvalidHeaders)?;
                            let value = ::http::HeaderValue::from_bytes(value.as_bytes())
                                .map_err(|_| FunctionResponsePollError::InvalidHeaders)?;

                            headers.push((name, value));
                        }

                        (body, status, headers)
                    };

                    let body = match body {
                        abi_http_capnp::http_response::body::Which::FunctionResourceId(resource_id) => HttpBody::for_function_stream(
                            store.data().self_instance.upgrade().ok_or(FunctionResponsePollError::AssertionError)?,
                            resource_id.into(),
                        ),
                        abi_http_capnp::http_response::body::Which::HostResourceId(resource_id) => store.data_mut().resource_set.http_bodies.remove(resource_id.into())
                            .ok_or(FunctionResponsePollError::HostResourceNotFound)?,
                    };

                    let mut http_response = http::Response::new(body);
                    *http_response.status_mut() = status;

                    for (name, value) in headers {
                        http_response.headers_mut().insert(name, value);
                    }

                    Ok(http_response)
                }),
                _other => Poll::Ready(Err(FunctionResponsePollError::AbiError)),
            }
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum FunctionInstanceInitError {
    #[error("function does not provide export that fx runtime expects to be present")]
    MissingExport,
    #[error("function does not provide memory export that fx runtime expects to be present")]
    MissingMemory,
    #[error("failed to create function instance because of unknown error")]
    UnknownError,
}

pub(crate) struct FunctionInstanceState {
    limits: wasmtime::StoreLimits,

    pub(crate) self_instance: send_wrapper::SendWrapper<Weak<FunctionInstance>>,

    pub(crate) waker: Option<std::task::Waker>,
    pub(crate) runtime_services: RuntimeServices,

    pub(crate) function_id: FunctionId,

    pub(crate) resource_set: FunctionResources,
    pub(crate) tasks_background: Vec<FunctionResourceId>,

    pub(crate) bindings: InstanceBindings,
    pub(crate) http_client: reqwest::Client,
    pub(crate) metrics: FunctionMetricsState,
}

impl FunctionInstanceState {
    pub fn new(
        runtime_services: RuntimeServices,
        limit_memory_bytes: Option<usize>,
        function_id: FunctionId,
        bindings: InstanceBindings,
    ) -> Self {
        let limits = wasmtime::StoreLimitsBuilder::new();

        let limits = match limit_memory_bytes {
            Some(limit_bytes) => limits.memory_size(limit_bytes).memories(1),
            None => limits,
        };

        Self {
            limits: limits.build(),

            self_instance: send_wrapper::SendWrapper::new(Weak::new()),

            waker: None,
            runtime_services,

            function_id,

            resource_set: FunctionResources::new(),
            tasks_background: Vec::new(),

            bindings,

            http_client: reqwest::Client::new(),
            metrics: FunctionMetricsState::new(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct RuntimeServices {
    pub(crate) local_worker: LocalWorkerController,
    pub(crate) logger: flume::Sender<LogMessageEvent>,
    pub(crate) sql: SqlController,
    pub(crate) kv: flume::Sender<KvMessage>,
    pub(crate) blob: flume::Sender<BlobMessage>,
}

impl RuntimeServices {
    pub fn new(
        local_worker: LocalWorkerController,
        logger: flume::Sender<LogMessageEvent>,
        sql: SqlController,
        kv: flume::Sender<KvMessage>,
        blob: flume::Sender<BlobMessage>,
    ) -> Self {
        Self {
            local_worker,
            logger,
            sql,
            kv,
            blob,
        }
    }
}

#[derive(Clone)]
pub(crate) struct InstanceBindings {
    pub(crate) env: HashMap<String, String>,
    pub(crate) sql: HashMap<String, SqlBindingConfig>,
    pub(crate) blob: HashMap<String, BlobBindingConfig>,
    pub(crate) kv: HashMap<String, KvBindingConfig>,
    pub(crate) functions: HashMap<String, FunctionBindingConfig>,
}

impl InstanceBindings {
    pub fn new(env: HashMap<String, String>, sql: HashMap<String, SqlBindingConfig>, blob: HashMap<String, BlobBindingConfig>, kv: HashMap<String, KvBindingConfig>, functions: HashMap<String, FunctionBindingConfig>) -> Self {
        Self {
            env,
            sql,
            blob,
            kv,
            functions,
        }
    }
}

pub(crate) struct FunctionFramePollFuture {
    instance: Rc<FunctionInstance>,
    resource_id: FunctionStreamResourceId,

    inner_poll_future: Option<LocalBoxFuture<'static, Poll<FunctionFramePollResult>>>,
}

type FunctionFramePollResult = Result<Option<FunctionFrame>, FunctionFramePollError>;

impl FunctionFramePollFuture {
    pub(crate) fn new(instance: Rc<FunctionInstance>, resource_id: FunctionStreamResourceId) -> Self {
        Self {
            instance,
            resource_id,
            inner_poll_future: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum FunctionFramePollError {
    #[error("failed to poll frame because of error in sdk on function side")]
    InternalSdkError,
    #[error("internal runtime assertion error")]
    AssertionError,
    #[error("function panicked when polling frame")]
    FunctionPanicked,
    #[error("function crashed when polling frame")]
    FunctionCrashed,
}

impl From<http_body_frame_poll::HttpBodyFramePollError> for FunctionFramePollError {
    fn from(err: http_body_frame_poll::HttpBodyFramePollError) -> Self {
        use http_body_frame_poll::HttpBodyFramePollError as SourceError;
        match err {
            SourceError::AbiError => Self::InternalSdkError,
            SourceError::AssertionError => Self::AssertionError,
            SourceError::FunctionPanicked => Self::FunctionPanicked,
            SourceError::FunctionCrashed => Self::FunctionCrashed,
        }
    }
}

impl Future for FunctionFramePollFuture {
    type Output = FunctionFramePollResult;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let instance = self.instance.clone();
        let resource_id = self.resource_id.clone();

        let result = self.inner_poll_future
            .get_or_insert_with(|| {
                async move {
                    let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
                    match instance.http_body_frame_poll(&resource_id, waker).await {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(Err(err)) => Poll::Ready(Err(FunctionFramePollError::from(err))),
                        Poll::Ready(Ok(v)) => Poll::Ready(Ok(v)),
                    }
                }.boxed_local()
            })
            .poll_unpin(cx);

        match result {
            Poll::Pending => {
                Poll::Pending
            },
            Poll::Ready(v) => {
                self.inner_poll_future = None;
                v
            },
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum WasmFunctionCallError {
    #[error("function panicked when called")]
    Panicked,
    #[error("function crashed when called")]
    Crashed,
}

impl From<wasmtime::Error> for WasmFunctionCallError {
    fn from(err: wasmtime::Error) -> Self {
        // TODO: forward backtraces to management thread (or logger thread)
        let trap = err.downcast::<wasmtime::Trap>();
        match trap {
            Ok(wasmtime::Trap::UnreachableCodeReached) => WasmFunctionCallError::Panicked,
            other => {
                error!("unexpected wasm trap when calling function: {other:?}");
                WasmFunctionCallError::Crashed
            }
        }
    }
}
