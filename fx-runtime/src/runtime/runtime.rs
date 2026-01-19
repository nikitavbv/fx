use {
    std::{
        sync::{Arc, Mutex, RwLock, atomic::{AtomicBool, AtomicU64, Ordering}},
        collections::{HashMap, VecDeque},
        ops::DerefMut,
        task::{self, Poll},
        time::{SystemTime, UNIX_EPOCH, Instant},
        io::Cursor,
    },
    tracing::{error, info},
    wasmtime::{AsContext, AsContextMut, Instance, Store},
    serde::{Serialize, Deserialize},
    futures::{FutureExt, TryFutureExt},
    rand::TryRngCore,
    parking_lot::ReentrantMutex,
    thiserror::Error,
    arc_swap::ArcSwap,
    fx_common::{
        LogMessage,
        DatabaseSqlQuery,
        DatabaseSqlBatchQuery,
        SqlResult,
        SqlResultRow,
        SqlValue,
        HttpRequestInternal,
        HttpResponse,
        FxExecutionError,
        FxFutureError,
        FxSqlError,
        SqlMigrations,
    },
    fx_types::{capnp, fx_capnp},
    crate::{
        common::{LogMessageEvent, LogSource},
        runtime::{
            kv::{KVStorage, NamespacedStorage, EmptyStorage, BoxedStorage, FsStorage, StorageError},
            error::{FxRuntimeError, FunctionInvokeError, FunctionInvokeInternalRuntimeError},
            sql::{self, SqlDatabase},
            futures::FuturesPool,
            streams::StreamsPool,
            metrics::Metrics,
            definition::{DefinitionProvider, FunctionDefinition, SqlStorageDefinition, DefinitionError},
            logs::{self, Logger, BoxLogger, StdoutLogger},
        },
    },
};

#[derive(Clone)]
pub struct FxRuntime {
    // TODO: re-enable forcing single thread later
    // _force_single_thread: std::marker::PhantomData<std::cell::Cell<()>>, // fx is designed to be single-threaded.

    pub engine: Arc<Engine>,
}

impl FxRuntime {
    pub fn new() -> Self {
        Self {
            // TODO: re-enable forcing single thread later
            // _force_single_thread: std::marker::PhantomData,

            engine: Arc::new(Engine::new()),
        }
    }

    pub fn with_code_storage(self, new_storage: BoxedStorage) -> Self {
        {
            let mut storage = self.engine.module_code_storage.write().unwrap();
            *storage = new_storage;
        }
        self
    }

    pub fn with_definition_provider(self, new_definition_provider: DefinitionProvider) -> Self {
        {
            let mut definition_provider = self.engine.definition_provider.write().unwrap();
            *definition_provider = new_definition_provider;
        }
        self
    }

    pub fn with_logger(self, new_logger: BoxLogger) -> Self {
        {
            let mut logger = self.engine.logger.write().unwrap();
            *logger = new_logger;
        }
        self
    }

    #[allow(dead_code)]
    pub async fn invoke_service<T: serde::ser::Serialize, S: serde::de::DeserializeOwned>(&self, service: &FunctionId, function_name: &str, argument: T) -> Result<(S, FunctionInvocationEvent), FunctionInvokeAndExecuteError> {
        self.engine.invoke_service(self.engine.clone(), service, function_name, argument).await
    }

    pub fn invoke_service_raw(&self, service: &FunctionId, function_name: &str, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FunctionInvokeError> {
        self.engine.invoke_service_raw(self.engine.clone(), service.clone(), function_name.to_owned(), argument)
    }

    #[allow(dead_code)]
    pub fn reload(&self, function_id: &FunctionId) {
        self.engine.reload(function_id)
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

/// There can be multiple instances of the same function (e.g, during redeploys).
/// ExecutionContext is "instance". FunctionId is "service name".
/// FunctionId points to one of execution contexts.
#[derive(Hash, Eq, PartialEq, Clone)]
pub struct ExecutionContextId {
    id: u64,
}

impl ExecutionContextId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }
}

pub struct Engine {
    wasmtime: wasmtime::Engine,

    pub metrics: Metrics,

    deployed_functions: RwLock<HashMap<FunctionId, ArcSwap<ExecutionContextId>>>,
    execution_context_id_counter: AtomicU64,
    pub execution_contexts: RwLock<HashMap<ExecutionContextId, Arc<ExecutionContext>>>,
    definition_provider: RwLock<DefinitionProvider>,

    // internal storage where .wasm is loaded from:
    module_code_storage: RwLock<BoxedStorage>,

    pub futures_pool: FuturesPool,
    pub streams_pool: StreamsPool,

    logger: RwLock<BoxLogger>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            wasmtime: wasmtime::Engine::default(),

            metrics: Metrics::new(),

            deployed_functions: RwLock::new(HashMap::new()),
            execution_context_id_counter: AtomicU64::new(0),
            execution_contexts: RwLock::new(HashMap::new()),
            definition_provider: RwLock::new(DefinitionProvider::new(BoxedStorage::new(EmptyStorage))),

            module_code_storage: RwLock::new(BoxedStorage::new(NamespacedStorage::new(b"services/", EmptyStorage))),

            futures_pool: FuturesPool::new(),
            streams_pool: StreamsPool::new(),

            logger: RwLock::new(BoxLogger::new(StdoutLogger::new())),
        }
    }

    pub async fn invoke_service<T: serde::ser::Serialize, S: serde::de::DeserializeOwned>(&self, engine: Arc<Engine>, service: &FunctionId, function_name: &str, argument: T) -> Result<(S, FunctionInvocationEvent), FunctionInvokeAndExecuteError> {
        let argument = rmp_serde::to_vec(&argument).unwrap();
        let (response, event) = self.invoke_service_raw(engine, service.clone(), function_name.to_owned(), argument)
            .map_err(|err| match err {
                FunctionInvokeError::RuntimeError(err) => {
                    error!("runtime error when invoking function: {err:?}");
                    FunctionInvokeAndExecuteError::RuntimeError
                },
                FunctionInvokeError::DefinitionMissing(err) => FunctionInvokeAndExecuteError::DefinitionMissing(err),
                FunctionInvokeError::CodeFailedToLoad(err) => FunctionInvokeAndExecuteError::CodeFailedToLoad(err),
                FunctionInvokeError::CodeNotFound => FunctionInvokeAndExecuteError::CodeNotFound,
                FunctionInvokeError::FailedToCompile(err) => FunctionInvokeAndExecuteError::FailedToCompile(err),
                // FunctionInvokeError::InstantionError(err) => FunctionInvokeAndExecuteError::InstantionError(err),
            })?
            .await
            .map_err(|err| match err {
                FunctionExecutionError::RuntimeError(err) => {
                    error!("runtime error when executing function: {err:?}");
                    FunctionInvokeAndExecuteError::RuntimeError
                }
                FunctionExecutionError::FunctionRuntimeError => FunctionInvokeAndExecuteError::FunctionRuntimeError,
                FunctionExecutionError::HandlerNotDefined => FunctionInvokeAndExecuteError::HandlerNotDefined,
                FunctionExecutionError::UserApplicationError { description } => FunctionInvokeAndExecuteError::UserApplicationError { description },
                FunctionExecutionError::FunctionPanicked => FunctionInvokeAndExecuteError::FunctionPanicked,
            })?;
        Ok((rmp_serde::from_slice(&response).unwrap(), event))
    }

    pub fn invoke_service_raw(&self, engine: Arc<Engine>, function_id: FunctionId, function_name: String, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FunctionInvokeError> {
        // TODO: will be able to remove this once switched to new server?
        let context_id = self.resolve_context_id_for_function(&function_id);

        let need_to_create_context = {
            let execution_contexts = match self.execution_contexts.read() {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to lock execution contexts: {err:?}");
                    return Err(FunctionInvokeError::from(
                        FunctionInvokeInternalRuntimeError::ExecutionContextsFailedToLock { reason: err.to_string() }
                    ));
                }
            };
            if let Some(context) = execution_contexts.get(&context_id) {
                context.needs_recreate.load(Ordering::SeqCst)
            } else {
                true
            }
        };

        if need_to_create_context {
            // need to create execution context first
            let mut execution_contexts = self.execution_contexts.write().unwrap();
            let ctx = execution_contexts.get(&context_id);
            if ctx.map(|v| v.needs_recreate.load(Ordering::SeqCst)).unwrap_or(true) {
                // TODO: will be able to remove this once switched to new server?
                let definition = self.definition_provider.read().unwrap().definition_for_function(&function_id)
                    .map_err(|err| FunctionInvokeError::DefinitionMissing(err))?;
                execution_contexts.insert((*context_id).clone(), Arc::new(self.create_execution_context(engine.clone(), &function_id, (*context_id).clone(), definition)?));
            }
        }

        Ok(self.run_service(engine, function_id.clone(), (*context_id).clone(), &function_name, argument))
    }

    pub fn reload(&self, function_id: &FunctionId) {
        let context_id = self.resolve_context_id_for_function(function_id);
        if let Some(execution_context) = self.execution_contexts.read().unwrap().get(&context_id) {
            println!("reloading {}", function_id.id);
            execution_context.needs_recreate.store(true, Ordering::SeqCst);
        }
    }

    fn run_service(&self, engine: Arc<Engine>, function_id: FunctionId, execution_context_id: ExecutionContextId, function_name: &str, argument: Vec<u8>) -> FunctionRuntimeFuture {
        FunctionRuntimeFuture {
            engine,
            function_id,
            execution_context_id,
            function_name: function_name.to_owned(),
            argument: argument.to_owned(),
            rpc_future_index: Arc::new(Mutex::new(None)),
        }
    }

    fn resolve_context_id_for_function(&self, function_id: &FunctionId) -> Arc<ExecutionContextId> {
        if let Some(context_id) = self.deployed_functions.read().unwrap().get(&function_id) {
            context_id.load().clone()
        } else {
            let mut deployed_functions = self.deployed_functions.write().unwrap();
            if let Some(context_id) = deployed_functions.get(&function_id) {
                context_id.load().clone()
            } else {
                let context_id = Arc::new(ExecutionContextId::new(self.execution_context_id_counter.fetch_add(1, Ordering::SeqCst)));
                deployed_functions.insert(function_id.clone(), ArcSwap::from(context_id.clone()));
                context_id
            }
        }
    }

    pub(crate) fn compile_module(&self, module_code: &[u8]) -> wasmtime::Module {
        wasmtime::Module::new(&self.wasmtime, module_code).unwrap()
    }

    pub(crate) fn create_execution_context_v2(
        &self,
        engine: Arc<Engine>,
        function_id: FunctionId,
        function_module: wasmtime::Module,
        sql: HashMap<String, SqlDatabase>,
        rpc: HashMap<String, RpcBinding>,
    ) -> ExecutionContextId {
        let execution_context_id = ExecutionContextId::new(self.execution_context_id_counter.fetch_add(1, Ordering::SeqCst));

        let execution_context = ExecutionContext::new(
            engine,
            function_id,
            execution_context_id.clone(),
            HashMap::new(),
            sql,
            rpc,
            function_module,
            true,
            true
        ).unwrap();
        self.execution_contexts.write().unwrap().insert(execution_context_id.clone(), Arc::new(execution_context));

        execution_context_id
    }

    pub(crate) fn remove_execution_context(&self, execution_context: &ExecutionContextId) {
        self.execution_contexts.write().unwrap().remove(execution_context);
    }

    pub(crate) fn update_function_execution_context(
        &self,
        function_id: FunctionId,
        execution_context_id: ExecutionContextId,
    ) -> Option<ExecutionContextId> {
        if let Some(target_context) = self.deployed_functions.read().unwrap().get(&function_id) {
            return Some(ExecutionContextId::new(target_context.swap(Arc::new(execution_context_id)).id));
        }

        let mut deployed_functions = self.deployed_functions.write().unwrap();
        if let Some(target_context) = deployed_functions.get(&function_id) {
            return Some(ExecutionContextId::new(target_context.swap(Arc::new(execution_context_id)).id));
        } else {
            deployed_functions.insert(function_id, ArcSwap::from_pointee(execution_context_id));
            return None;
        }
    }

    fn create_execution_context(
        &self,
        engine: Arc<Engine>,
        function_id: &FunctionId,
        execution_context_id: ExecutionContextId,
        definition: FunctionDefinition,
    ) -> Result<ExecutionContext, FunctionInvokeError> {
        let memory_tracker = crate::runtime::profiling::init_memory_tracker();

        let module_code = self.module_code_storage.read().unwrap().get(function_id.id.as_bytes())
            .map_err(|err| FunctionInvokeError::CodeFailedToLoad(err))?;
        let module_code = match module_code {
            Some(v) => v,
            None => return Err(FunctionInvokeError::CodeNotFound),
        };

        // TODO: kv should not be created here. Instead, it should be lazily created when a KV api call is made.
        // main reasoning for that is if we failed to connect to storage, we should continue with function
        // initialization. Then function can handle binding errors on its own.
        let mut kv = HashMap::new();
        for kv_definition in definition.kv {
            kv.insert(
                kv_definition.id,
                BoxedStorage::new(FsStorage::new(kv_definition.path.into()).unwrap()),
            );
        }

        // TODO: sql should be created lazily. See comment above for reasoning
        let mut sql = HashMap::new();
        for sql_definition in definition.sql {
            sql.insert(
                sql_definition.id,
                match sql_definition.storage {
                    SqlStorageDefinition::InMemory => SqlDatabase::in_memory().unwrap(), // TODO: function crashes, all data is lost
                    SqlStorageDefinition::Path(path) => SqlDatabase::new(path)
                        .unwrap()
                },
            );
        }

        // TODO: creating a list of rpc bindings like this is probably not needed
        let mut rpc = HashMap::new();
        for rpc_definition in definition.rpc {
            rpc.insert(
                rpc_definition.id.clone(),
                RpcBinding {
                    target_function: FunctionId::new(rpc_definition.id),
                },
            );
        }

        let execution_context = ExecutionContext::new(
            engine.clone(),
            function_id.clone(),
            execution_context_id,
            kv,
            sql,
            rpc,
            self.compile_module(&module_code),
            true, // TODO: permissions
            true, // TODO: permissions
        );

        if let Some(memory_usage) = memory_tracker.report_total() {
            engine.metrics.function_execution_context_init_memory_usage.with_label_values(&[function_id.as_string()]).set(memory_usage as i64);
        }

        execution_context
    }

    pub(crate) fn stream_poll_next(&self, execution_context_id: &ExecutionContextId, index: i64) -> Poll<Option<Result<Vec<u8>, FxRuntimeError>>> {
        let ctxs = self.execution_contexts.read().unwrap();
        let ctx = ctxs.get(execution_context_id).unwrap();
        let mut store_lock = ctx.store.lock().unwrap();

        // TODO: measure points
        function_stream_poll_next(&mut store_lock, index as u64)
            .map(|v| Ok(v).transpose())
    }

    pub(crate) fn stream_drop(&self, execution_context_id: &ExecutionContextId, index: i64) {
        let ctxs = self.execution_contexts.read().unwrap();
        let ctx = ctxs.get(execution_context_id).unwrap();
        ctx.streams_to_drop.lock().unwrap().push_back(index);
    }

    pub fn log(&self, message: LogMessageEvent) {
        self.logger.read().unwrap().log(message);
    }
}

#[derive(Error, Debug)]
pub enum FunctionInvokeAndExecuteError {
    /// Failed to invoke function because of internal error in runtime implementation.
    /// Should never happen. Getting this error means there is a bug somewhere.
    #[error("runtime internal error")]
    RuntimeError,

    /// Failed to poll future because of internal error in runtime implementation within function.
    /// Getting this error could mean there is a bug in function's sdk or that the function does not
    /// behave properly.
    #[error("function runtime internal error")]
    FunctionRuntimeError,

    /// Failed to execute function because of user application error.
    #[error("user application error: {description:?}")]
    UserApplicationError {
        description: String,
    },

    /// Failed to execute function because user code panicked during execution.
    #[error("function panicked")]
    FunctionPanicked,

    /// Failed to execute function because handler with this name is not defined.
    #[error("handler with this name is not defined")]
    HandlerNotDefined,

    /// Definitions are required in order to create an instance of function, so invocation
    /// will fail if definition failed to load.
    #[error("failed to get definition: {0:?}")]
    DefinitionMissing(DefinitionError),

    /// Function cannot be invoked if runtime failed to load its code
    #[error("failed to load code: {0:?}")]
    CodeFailedToLoad(StorageError),

    /// Function cannot be invoked if runtime could not find its code in storage
    #[error("code not found")]
    CodeNotFound,

    /// Function cannot be invoked if it failed to compile
    #[error("failed to compile: {0:?}")]
    FailedToCompile(CompilerError),
}

/// Error that may be returned when function is being executed (i.e, when FunctionRuntimeFuture is being polled)
#[derive(Error, Debug)]
pub enum FunctionExecutionError {
    /// Failed to execute function because of internal error in runtime implementation.
    /// Should never happen. Getting this error means there is a bug somewhere.
    #[error("runtime internal error: {0:?}")]
    RuntimeError(#[from] FunctionExecutionInternalRuntimeError),

    /// Failed to execute function because of internal error in runtime implementation on function side.
    /// Should not happen normally. Getting this error could mean there is a bug in function's sdk or
    /// that the function does not behave properly.
    #[error("function runtime internal error")]
    FunctionRuntimeError,

    /// Failed to execute function because of user application error.
    #[error("user application error: {description:?}")]
    UserApplicationError {
        description: String,
    },

    /// panic in user code interrupted the execution
    #[error("function panicked")]
    FunctionPanicked,

    /// Failed to execute function because handler with this name is not defined.
    #[error("handler with this name is not defined")]
    HandlerNotDefined,
}

/// Failed to execute function because of internal error in runtime implementation.
/// Should never happen. Getting this error means there is a bug somewhere.
#[derive(Error, Debug)]
pub enum FunctionExecutionInternalRuntimeError {
    #[error("failed to lock function store")]
    StoreFailedToLock,
}

pub struct FunctionRuntimeFuture {
    engine: Arc<Engine>,
    function_id: FunctionId,
    execution_context_id: ExecutionContextId,
    function_name: String,
    argument: Vec<u8>,
    rpc_future_index: Arc<Mutex<Option<i64>>>,
}

impl FunctionRuntimeFuture {
    fn record_function_invocation(&self) {
        /*let engine = self.engine.clone();
        let message = FunctionInvokeEvent {
            function_id: self.ctx.service_id.id.clone(),
        };
        tokio::runtime::Handle::current().spawn(async move {
            engine.push_to_queue(QUEUE_SYSTEM_INVOCATIONS, message).await;
        });*/
    }
}

impl Future for FunctionRuntimeFuture {
    type Output = Result<(Vec<u8>, FunctionInvocationEvent), FunctionExecutionError>;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let started_at = Instant::now();

        let rpc_future_index = self.rpc_future_index.clone();
        let mut rpc_future_index = rpc_future_index.lock().unwrap();

        let argument = self.argument.clone();
        let function_name = self.function_name.clone();
        let ctx = {
            let ctxs = self.engine.execution_contexts.read().unwrap();
            ctxs.get(&self.execution_context_id).unwrap().clone()
        };
        let mut store = match ctx.store.lock() {
            Ok(v) => v,
            Err(err) => {
                error!("failed to lock ctx.store: {err:?}");
                return std::task::Poll::Ready(Err(FunctionExecutionError::from(
                    FunctionExecutionInternalRuntimeError::StoreFailedToLock
                )));
            }
        };

        {
            let function_env = store.data();
            if function_env.execution_context.read().unwrap().is_none() {
                let mut f_env_execution_context = function_env.execution_context.write().unwrap();
                *f_env_execution_context = Some(ctx.clone());
            }
        }

        // cleanup futures
        if let Ok(mut futures_to_drop) = ctx.futures_to_drop.try_lock() {
            while let Some(future_to_drop) = futures_to_drop.pop_front() {
                function_future_drop(&mut store, future_to_drop as u64);
            }
        }

        // cleanup streams
        if let Ok(mut streams_to_drop) = ctx.streams_to_drop.try_lock() {
            while let Some(stream_to_drop) = streams_to_drop.pop_front() {
                function_stream_drop(&mut store, stream_to_drop as u64);
            }
        }

        // poll this future
        if let Some(rpc_future_index_value) = rpc_future_index.as_ref().clone() {
            // TODO: measure points
            let future_poll_result = function_future_poll(&mut store, *rpc_future_index_value as u64)
                .map_err(|err| match err {
                    FunctionFuturePollApiError::FunctionRuntimeError => FunctionExecutionError::FunctionRuntimeError,
                    FunctionFuturePollApiError::UserApplicationError { description } => FunctionExecutionError::UserApplicationError { description },
                    FunctionFuturePollApiError::FunctionPanicked => FunctionExecutionError::FunctionPanicked,
                })?;
            match future_poll_result {
                Poll::Pending => Poll::Pending,
                Poll::Ready(response) => {
                    *rpc_future_index = None;
                    self.record_function_invocation();
                    Poll::Ready(Ok((response, FunctionInvocationEvent {
                    })))
                }
            }
        } else {
            store.data_mut().futures_waker = Some(cx.waker().clone());
            store.data_mut().instance = Some(ctx.instance.clone());

            let points_before = u64::MAX;
            // store.set_fuel(points_before).unwrap();

            // TODO: fx api instead of all of this
            let future_index = match function_invoke(&mut store, function_name, argument) {
                Ok(v) => v,
                Err(err) => return Poll::Ready(Err(match err {
                    FunctionInvokeApiError::HandlerNotDefined => {
                        // recreating context would not help in this case, so we are not doing that
                        FunctionExecutionError::HandlerNotDefined
                    },
                    FunctionInvokeApiError::FunctionPanicked => {
                        // panics are usually not recoverable, so we need to re-create context or following
                        // invocations will also fail
                        ctx.needs_recreate.store(true, Ordering::SeqCst);
                        FunctionExecutionError::FunctionPanicked
                    }
                })),
            };

            let result = match function_future_poll(&mut store, future_index as u64) {
                Ok(v) => v,
                Err(err) => return std::task::Poll::Ready(Err(match err {
                    FunctionFuturePollApiError::FunctionRuntimeError => {
                        // recoverable error, internal runtime error does not mean that function crashed
                        FunctionExecutionError::FunctionRuntimeError
                    },
                    FunctionFuturePollApiError::UserApplicationError { description } => {
                        // user application error is recoverable, no need to re-create context
                        FunctionExecutionError::UserApplicationError { description }
                    },
                    FunctionFuturePollApiError::FunctionPanicked => {
                        // panics are usually not recoverable, so we need to re-create context or following
                        // invocations will also fail
                        ctx.needs_recreate.store(true, Ordering::SeqCst);
                        FunctionExecutionError::FunctionPanicked
                    }
                }))
            };
            let result = match result {
                Poll::Pending => {
                    *rpc_future_index = Some(future_index as i64);
                    std::task::Poll::Pending
                },
                Poll::Ready(response) => {
                    std::task::Poll::Ready(Ok((response, FunctionInvocationEvent {
                    })))
                }
            };

            // TODO: record fuel used

            if result.is_ready() {
                self.record_function_invocation();
            }

            self.engine.metrics.function_poll_time.with_label_values(&[&self.function_id.id]).inc_by((Instant::now() - started_at).as_millis() as u64);

            result
        }
    }
}

impl Drop for FunctionRuntimeFuture {
    fn drop(&mut self) {
        let rpc_future_index = match self.rpc_future_index.try_lock() {
            Ok(v) => v,
            Err(err) => {
                error!("failed to lock rpc_future_index when dropping FunctionRuntimeFuture: {err:?}");
                return;
            }
        };

        let rpc_future_index = match rpc_future_index.as_ref() {
            Some(v) => v,
            None => return,
        };

        let ctx = {
            let ctxs = self.engine.execution_contexts.try_read().unwrap();
            ctxs.get(&self.execution_context_id).unwrap().clone()
        };
        ctx.futures_to_drop.lock().unwrap().push_back(*rpc_future_index);
    }
}

pub(crate) struct ExecutionContext {
    pub(crate) function_id: FunctionId,
    pub(crate) instance: Instance,
    pub(crate) store: Arc<Mutex<Store<ExecutionEnv>>>,
    // pub(crate) function_env: FunctionEnv<ExecutionEnv>,
    // TODO: regular boolean can probably be used here
    needs_recreate: Arc<AtomicBool>,
    futures_to_drop: Arc<Mutex<VecDeque<i64>>>,
    streams_to_drop: Arc<Mutex<VecDeque<i64>>>,
}

impl ExecutionContext {
    pub fn new(
        engine: Arc<Engine>,
        function_id: FunctionId,
        execution_context_id: ExecutionContextId,
        storage: HashMap<String, BoxedStorage>,
        sql: HashMap<String, SqlDatabase>,
        rpc: HashMap<String, RpcBinding>,
        module: wasmtime::Module,
        allow_fetch: bool,
        allow_log: bool
    ) -> Result<Self, FunctionInvokeError> {
        let execution_env = ExecutionEnv::new(engine.clone(), function_id.clone(), execution_context_id, storage.clone(), sql.clone(), rpc.clone(), allow_fetch, allow_log);

        let mut linker = wasmtime::Linker::new(&engine.wasmtime);
        linker.func_wrap("fx", "fx_api", crate::runtime::api::fx_api_handler).unwrap();

        let mut store = wasmtime::Store::new(&engine.wasmtime, execution_env);
        let instance = linker.instantiate(&mut store, &module).unwrap();

        let memory = instance.get_memory(&mut store, "memory").unwrap();
        store.data_mut().memory = Some(memory.clone());

        // some libraries, like leptos, have wbidgen imports, but do not use them. Let's add them here so that module can be linked
        /*for import in module.imports().into_iter() {
            let module = import.module();
            if module != "fx" {
                match import.ty() {
                    wasmer::ExternType::Function(f) => {
                        import_object.define(module, import.name(), Function::new_with_env(&mut store, &function_env, f, crate::api::unsupported::handle_unsupported));
                    },
                    other => panic!("unexpected import type: {other:?}"),
                }
            }
        }*/

        Ok(Self {
            function_id,
            instance,
            store: Arc::new(Mutex::new(store)),
            needs_recreate: Arc::new(AtomicBool::new(false)),
            futures_to_drop: Arc::new(Mutex::new(VecDeque::new())),
            streams_to_drop: Arc::new(Mutex::new(VecDeque::new())),
        })
    }
}

pub(crate) struct ExecutionEnv {
    execution_context: RwLock<Option<Arc<ExecutionContext>>>,
    pub(crate) execution_context_id: ExecutionContextId,

    pub(crate) futures_waker: Option<std::task::Waker>,

    pub(crate) engine: Arc<Engine>,
    pub(crate) instance: Option<Instance>,
    pub(crate) memory: Option<wasmtime::Memory>,
    pub(crate) execution_error: Option<Vec<u8>>,
    pub(crate) rpc_response: Option<Vec<u8>>,

    pub(crate) function_id: FunctionId,

    pub(crate) storage: HashMap<String, BoxedStorage>,
    pub(crate) sql: HashMap<String, SqlDatabase>,
    pub(crate) rpc: HashMap<String, RpcBinding>,

    allow_fetch: bool,
    allow_log: bool,

    pub(crate) fetch_client: reqwest::Client,
}

impl ExecutionEnv {
    pub fn new(
        engine: Arc<Engine>,
        function_id: FunctionId,
        execution_context_id: ExecutionContextId,
        storage: HashMap<String, BoxedStorage>,
        sql: HashMap<String, SqlDatabase>,
        rpc: HashMap<String, RpcBinding>,
        allow_fetch: bool,
        allow_log: bool,
    ) -> Self {
        Self {
            execution_context: RwLock::new(None),
            execution_context_id,
            futures_waker: None,
            engine,
            instance: None,
            memory: None,
            execution_error: None,
            rpc_response: None,
            function_id,
            storage,
            sql,
            rpc,
            allow_fetch,
            allow_log,
            fetch_client: reqwest::Client::new(),
        }
    }
}

/// Error that occured while calling invoke api of the function
#[derive(Error, Debug)]
enum FunctionInvokeApiError {
    /// Handler with this name is not defined in the function
    #[error("handler with this name is not defined")]
    HandlerNotDefined,

    /// Failed to invoke function because it panicked
    #[error("function panicked")]
    FunctionPanicked
}

fn function_invoke(ctx: &mut Store<ExecutionEnv>, handler: String, payload: Vec<u8>) -> Result<u64, FunctionInvokeApiError> {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_function_api_call::Builder>();
    let op = request.init_op();
    let mut function_invoke_request = op.init_invoke();
    function_invoke_request.set_method(handler);
    function_invoke_request.set_payload(&payload);

    let response = invoke_fx_api(ctx, message)
        .map_err(|err| match err {
            FunctionApiError::FunctionPanicked => FunctionInvokeApiError::FunctionPanicked,
        })?;
    let response = response.get_root::<fx_capnp::fx_function_api_call_result::Reader>().unwrap();
    match response.get_op().which().unwrap() {
        fx_capnp::fx_function_api_call_result::op::Which::Invoke(v) => {
            let invoke_response = v.unwrap();
            match invoke_response.get_result().which().unwrap() {
                fx_capnp::function_invoke_response::result::Which::FutureId(v) => {
                    Ok(v)
                },
                fx_capnp::function_invoke_response::result::Which::Error(err) => {
                    let err = err.unwrap().get_error();
                    match err.which().unwrap() {
                        fx_capnp::function_invoke_error::error::Which::HandlerNotFound(_v) => Err(FunctionInvokeApiError::HandlerNotDefined),
                        _other => panic!("error when invoking function"),
                    }
                }
            }
        }
        _other => panic!("unexpected response from function invoke api"),
    }
}

/// Error that occurred while calling future_poll api of the function
#[derive(Error, Debug)]
enum FunctionFuturePollApiError {
    /// Failed to poll future because of internal error in runtime implementation within function.
    /// Getting this error could mean there is a bug in function's sdk or that the function does not
    /// behave properly.
    #[error("function runtime internal error")]
    FunctionRuntimeError,

    /// When future was polled, an error was returned by the user application code
    #[error("user application error: {description}")]
    UserApplicationError {
        description: String,
    },

    /// Failed to poll future because function panicked
    #[error("function panicked")]
    FunctionPanicked,
}

fn function_future_poll(ctx: &mut Store<ExecutionEnv>, future_id: u64) -> Result<Poll<Vec<u8>>, FunctionFuturePollApiError> {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_function_api_call::Builder>();
    let op = request.init_op();
    let mut future_poll_request = op.init_future_poll();
    future_poll_request.set_future_id(future_id);

    let response = invoke_fx_api(ctx, message)
        .map_err(|err| match err {
            FunctionApiError::FunctionPanicked => FunctionFuturePollApiError::FunctionPanicked,
        })?;
    let response = response.get_root::<fx_capnp::fx_function_api_call_result::Reader>().unwrap();

    Ok(match response.get_op().which().unwrap() {
        fx_capnp::fx_function_api_call_result::op::Which::FuturePoll(v) => {
            let future_poll_response = v.unwrap();
            match future_poll_response.get_response().which().unwrap() {
                fx_capnp::function_future_poll_response::response::Which::Pending(_) => Poll::Pending,
                fx_capnp::function_future_poll_response::response::Which::Ready(v) => {
                    Poll::Ready(v.unwrap().to_vec())
                },
                fx_capnp::function_future_poll_response::response::Which::Error(error) => {
                    let error = error.unwrap();
                    // TODO: refactor FxRuntimeError into something more granular
                    return Err(match error.get_error().which().unwrap() {
                        fx_capnp::function_future_poll_error::error::Which::InternalRuntimeError(_v) => FunctionFuturePollApiError::FunctionRuntimeError,
                        fx_capnp::function_future_poll_error::error::Which::UserApplicationError(v) => FunctionFuturePollApiError::UserApplicationError {
                            description: v.unwrap().to_string().unwrap(),
                        },
                    });
                }
            }
        },
        _other => panic!("unexpected response from future_poll api"),
    })
}

fn function_future_drop(ctx: &mut Store<ExecutionEnv>, future_id: u64) {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_function_api_call::Builder>();
    let op = request.init_op();
    let mut future_drop_request = op.init_future_drop();
    future_drop_request.set_future_id(future_id);

    let _response = invoke_fx_api(ctx, message).unwrap();
}

fn function_stream_poll_next(ctx: &mut Store<ExecutionEnv>, stream_id: u64) -> Poll<Option<Vec<u8>>> {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_function_api_call::Builder>();
    let op = request.init_op();
    let mut stream_poll_next_request = op.init_stream_poll_next();
    stream_poll_next_request.set_stream_id(stream_id);

    let response = invoke_fx_api(ctx, message).unwrap();
    let response = response.get_root::<fx_capnp::fx_function_api_call_result::Reader>().unwrap();

    match response.get_op().which().unwrap() {
        fx_capnp::fx_function_api_call_result::op::Which::StreamPollNext(v) => {
            let stream_poll_next_response = v.unwrap();
            match stream_poll_next_response.get_response().which().unwrap() {
                fx_capnp::function_stream_poll_next_response::response::Which::Pending(_) => Poll::Pending,
                fx_capnp::function_stream_poll_next_response::response::Which::Ready(v) => {
                    Poll::Ready(Some(v.unwrap().to_vec()))
                },
                fx_capnp::function_stream_poll_next_response::response::Which::Finished(_) => Poll::Ready(None),
            }
        },
        _other => panic!("unexpected response from stream_poll_next api"),
    }
}

fn function_stream_drop(ctx: &mut Store<ExecutionEnv>, stream_id: u64) {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_function_api_call::Builder>();
    let op = request.init_op();
    let mut stream_drop_request = op.init_stream_drop();
    stream_drop_request.set_stream_id(stream_id);

    let _response = invoke_fx_api(ctx, message).unwrap();
}

/// Error that occurred while communicating with function api
#[derive(Error, Debug)]
enum FunctionApiError {
    /// Api request failed because function code panicked
    #[error("function panicked")]
    FunctionPanicked,
}

pub(crate) fn invoke_fx_api(
    ctx: &mut Store<ExecutionEnv>,
    message: capnp::message::Builder<capnp::message::HeapAllocator>
) -> Result<capnp::message::Reader<capnp::serialize::OwnedSegments>, FunctionApiError> {
    let instance = ctx.data().instance.as_ref().unwrap().clone();
    let memory = instance.get_memory(ctx.as_context_mut(), "memory").unwrap();

    let response_ptr = {
        let message_size = capnp::serialize::compute_serialized_size_in_words(&message) * 8;
        let ptr = instance.get_typed_func::<i64, i64>(ctx.as_context_mut(), "_fx_malloc").unwrap()
            .call(ctx.as_context_mut(), message_size as i64)
            .unwrap() as usize;

        let view = memory.data_mut(ctx.as_context_mut());

        unsafe {
            capnp::serialize::write_message(
                &mut view[ptr..ptr+message_size],
                &message
            ).unwrap();
        }

        instance.get_typed_func::<(i64, i64), i64>(ctx.as_context_mut(), "_fx_api").unwrap()
            .call(ctx.as_context_mut(), (ptr as i64, message_size as i64))
            .map_err(|err| {
                let trap = err.downcast::<wasmtime::Trap>().unwrap();
                match trap {
                    wasmtime::Trap::UnreachableCodeReached => FunctionApiError::FunctionPanicked,
                    other => panic!("unexpected trap: {other:?}"),
                }
            })? as usize
    };

    let header_length = 4;
    let (response, response_length) = {
        let view = memory.data(ctx.as_context());
        let header_bytes = {
            &view[response_ptr..response_ptr+header_length]
        };
        let response_length = u32::from_le_bytes(header_bytes.try_into().unwrap());

        let response = {
            let response_length = response_length as usize;
            &view[(response_ptr + header_length)..(response_ptr + header_length + response_length)]
        };

        (response.to_vec(), response_length)
    };

    instance.get_typed_func::<(i64, i64), ()>(ctx.as_context_mut(), "_fx_dealloc").unwrap()
        .call(ctx.as_context_mut(), (response_ptr as i64, (header_length + response_length as usize) as i64))
        .unwrap();

    Ok(capnp::serialize::read_message(Cursor::new(response), capnp::message::ReaderOptions::default()).unwrap())
}

pub fn write_memory_obj<T: Sized>(memory: &mut [u8], addr: i64, obj: T) {
    write_memory(memory, addr, unsafe { std::slice::from_raw_parts(&obj as *const T as *const u8, std::mem::size_of_val(&obj)) });
}

pub fn write_memory(ctx: &mut [u8], addr: i64, value: &[u8]) {
    let start = addr as usize;
    ctx[start..start + value.len()].copy_from_slice(value);
}

#[repr(C)]
pub(crate) struct PtrWithLen {
    pub ptr: i64,
    pub len: i64,
}

#[derive(Clone)]
pub(crate) struct RpcBinding {
    pub(crate) target_function: FunctionId,
}

impl RpcBinding {
    pub fn new(target_function: FunctionId) -> Self {
        Self { target_function }
    }
}

pub struct FunctionInvocationEvent {
}

#[derive(Error, Debug)]
pub enum CompilerError {}
