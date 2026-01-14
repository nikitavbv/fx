use {
    std::{
        sync::{Arc, Mutex, RwLock, atomic::{AtomicBool, Ordering}},
        collections::{HashMap, VecDeque},
        ops::DerefMut,
        task::{self, Poll},
        time::{SystemTime, UNIX_EPOCH, Instant},
        io::Cursor,
    },
    tracing::{error, info},
    wasmer::{
        wasmparser::Operator,
        sys::{Cranelift, CompilerConfig, EngineBuilder},
        Store,
        FunctionEnv,
        FunctionEnvMut,
        Memory,
        Instance,
        Function,
        Value,
        imports,
        ExportError,
    },
    wasmer_middlewares::{Metering, metering::{get_remaining_points, set_remaining_points, MeteringPoints}},
    serde::{Serialize, Deserialize},
    futures::{FutureExt, TryFutureExt},
    rand::TryRngCore,
    parking_lot::ReentrantMutex,
    thiserror::Error,
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
    fx_runtime_common::{LogMessageEvent, LogSource},
    fx_api::{capnp, fx_capnp},
    crate::{
        kv::{KVStorage, NamespacedStorage, EmptyStorage, BoxedStorage, FsStorage, StorageError},
        error::{FxRuntimeError, FunctionInvokeError, FunctionInvokeInternalRuntimeError},
        sql::{self, SqlDatabase},
        compiler::{Compiler, BoxedCompiler, CraneliftCompiler, CompilerMetadata, CompilerError},
        futures::FuturesPool,
        streams::StreamsPool,
        metrics::Metrics,
        definition::{DefinitionProvider, FunctionDefinition, SqlStorageDefinition, DefinitionError},
        logs::{self, Logger, BoxLogger, StdoutLogger},
    },
};

#[derive(Clone)]
pub struct FxRuntime {
    _force_single_thread: std::marker::PhantomData<std::cell::Cell<()>>, // fx is designed to be single-threaded.

    pub engine: Arc<Engine>,
}

impl FxRuntime {
    pub fn new() -> Self {
        Self {
            _force_single_thread: std::marker::PhantomData,

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

    #[allow(dead_code)]
    pub fn with_compiler(self, new_compiler: BoxedCompiler) -> Self {
        {
            let mut compiler = self.engine.compiler.write().unwrap();
            *compiler = new_compiler;
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

#[derive(Hash, Eq, PartialEq, Clone, Serialize, Deserialize)]
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

pub struct Engine {
    pub metrics: Metrics,

    compiler: RwLock<BoxedCompiler>,

    pub execution_contexts: RwLock<HashMap<FunctionId, Arc<ExecutionContext>>>,
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
            metrics: Metrics::new(),

            compiler: RwLock::new(BoxedCompiler::new(CraneliftCompiler::new())),

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
                FunctionInvokeError::InstantionError(err) => FunctionInvokeAndExecuteError::InstantionError(err),
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
                FunctionExecutionError::FunctionPanicked { message } => FunctionInvokeAndExecuteError::FunctionPanicked { message },
            })?;
        Ok((rmp_serde::from_slice(&response).unwrap(), event))
    }

    pub fn invoke_service_raw(&self, engine: Arc<Engine>, function_id: FunctionId, function_name: String, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FunctionInvokeError> {
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
            if let Some(context) = execution_contexts.get(&function_id) {
                context.needs_recreate.load(Ordering::SeqCst)
            } else {
                true
            }
        };

        if need_to_create_context {
            // need to create execution context first
            let mut execution_contexts = self.execution_contexts.write().unwrap();
            let ctx = execution_contexts.get(&function_id);
            if ctx.map(|v| v.needs_recreate.load(Ordering::SeqCst)).unwrap_or(true) {
                let definition = self.definition_provider.read().unwrap().definition_for_function(&function_id)
                    .map_err(|err| FunctionInvokeError::DefinitionMissing(err))?;
                execution_contexts.insert(function_id.clone(), Arc::new(self.create_execution_context(engine.clone(), &function_id, definition)?));
            }
        }

        Ok(self.run_service(engine, function_id.clone(), &function_name, argument))
    }

    pub fn reload(&self, function_id: &FunctionId) {
        if let Some(execution_context) = self.execution_contexts.read().unwrap().get(function_id) {
            println!("reloading {}", function_id.id);
            execution_context.needs_recreate.store(true, Ordering::SeqCst);
        }
    }

    fn run_service(&self, engine: Arc<Engine>, function_id: FunctionId, function_name: &str, argument: Vec<u8>) -> FunctionRuntimeFuture {
        FunctionRuntimeFuture {
            engine,
            function_id,
            function_name: function_name.to_owned(),
            argument: argument.to_owned(),
            rpc_future_index: Arc::new(Mutex::new(None)),
        }
    }

    fn create_execution_context(&self, engine: Arc<Engine>, function_id: &FunctionId, definition: FunctionDefinition) -> Result<ExecutionContext, FunctionInvokeError> {
        let memory_tracker = crate::profiling::init_memory_tracker();

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
                rpc_definition.id,
                RpcBinding {},
            );
        }

        let execution_context = ExecutionContext::new(
            engine.clone(),
            function_id.clone(),
            kv,
            sql,
            rpc,
            module_code,
            true, // TODO: permissions
            true, // TODO: permissions
        );

        if let Some(memory_usage) = memory_tracker.report_total() {
            engine.metrics.function_execution_context_init_memory_usage.with_label_values(&[function_id.as_string()]).set(memory_usage as i64);
        }

        execution_context
    }

    pub(crate) fn stream_poll_next(&self, function_id: &FunctionId, index: i64) -> Poll<Option<Result<Vec<u8>, FxRuntimeError>>> {
        let ctxs = self.execution_contexts.read().unwrap();
        let ctx = ctxs.get(function_id).unwrap();
        let mut store_lock = ctx.store.lock().unwrap();
        let store = store_lock.deref_mut();

        // TODO: measure points
        function_stream_poll_next(&mut ctx.function_env.clone().into_mut(store), index as u64)
            .map(|v| Ok(v).transpose())
    }

    pub(crate) fn stream_drop(&self, function_id: &FunctionId, index: i64) {
        let ctxs = self.execution_contexts.read().unwrap();
        let ctx = ctxs.get(function_id).unwrap();
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
    #[error("function panicked: {message:?}")]
    FunctionPanicked {
        message: String,
    },

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

    /// WASM engine could not create instance
    #[error("failed to create instance: {0:?}")]
    InstantionError(wasmer::InstantiationError),
}

pub struct FunctionRuntimeFuture {
    engine: Arc<Engine>,
    function_id: FunctionId,
    function_name: String,
    argument: Vec<u8>,
    rpc_future_index: Arc<Mutex<Option<i64>>>,
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
    FunctionPanicked {
        message: String,
    },

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
            ctxs.get(&self.function_id).unwrap().clone()
        };
        let mut store_lock = match ctx.store.lock() {
            Ok(v) => v,
            Err(err) => {
                error!("failed to lock ctx.store: {err:?}");
                return std::task::Poll::Ready(Err(FunctionExecutionError::from(
                    FunctionExecutionInternalRuntimeError::StoreFailedToLock
                )));
            }
        };
        let store = store_lock.deref_mut();

        {
            let function_env = ctx.function_env.as_ref(store);
            if function_env.execution_context.read().unwrap().is_none() {
                let mut f_env_execution_context = function_env.execution_context.write().unwrap();
                *f_env_execution_context = Some(ctx.clone());
            }
        }

        // cleanup futures
        if let Ok(mut futures_to_drop) = ctx.futures_to_drop.try_lock() {
            while let Some(future_to_drop) = futures_to_drop.pop_front() {
                function_future_drop(&mut ctx.function_env.clone().into_mut(store), future_to_drop as u64);
            }
        }

        // cleanup streams
        if let Ok(mut streams_to_drop) = ctx.streams_to_drop.try_lock() {
            while let Some(stream_to_drop) = streams_to_drop.pop_front() {
                function_stream_drop(&mut ctx.function_env.clone().into_mut(store), stream_to_drop as u64);
            }
        }

        // poll this future
        if let Some(rpc_future_index_value) = rpc_future_index.as_ref().clone() {
            // TODO: measure points
            let mut function_env_mut = ctx.function_env.clone().into_mut(store);
            let future_poll_result = function_future_poll(&mut function_env_mut, *rpc_future_index_value as u64)
                .map_err(|err| match err {
                    FunctionFuturePollApiError::FunctionRuntimeError => FunctionExecutionError::FunctionRuntimeError,
                    FunctionFuturePollApiError::UserApplicationError { description } => FunctionExecutionError::UserApplicationError { description },
                    FunctionFuturePollApiError::FunctionPanicked { message } => FunctionExecutionError::FunctionPanicked { message },
                })?;
            match future_poll_result {
                Poll::Pending => Poll::Pending,
                Poll::Ready(response) => {
                    *rpc_future_index = None;
                    let compiler_metadata = ctx.function_env.as_ref(store).compiler_metadata.clone();
                    drop(store_lock);
                    self.record_function_invocation();
                    Poll::Ready(Ok((response, FunctionInvocationEvent {
                        compiler_metadata,
                    })))
                }
            }
        } else {
            ctx.function_env.as_mut(store).futures_waker = Some(cx.waker().clone());

            let points_before = u64::MAX;
            set_remaining_points(store, &ctx.instance, points_before);

            let memory = ctx.instance.exports.get_memory("memory").unwrap();
            ctx.function_env.as_mut(store).instance = Some(ctx.instance.clone());
            ctx.function_env.as_mut(store).memory = Some(memory.clone());

            // TODO: fx api instead of all of this
            let mut function_env_mut = ctx.function_env.clone().into_mut(store);
            let future_index = match function_invoke(&mut function_env_mut, function_name, argument) {
                Ok(v) => v,
                Err(err) => return Poll::Ready(Err(match err {
                    FunctionInvokeApiError::HandlerNotDefined => {
                        // recreating context would not help in this case, so we are not doing that
                        FunctionExecutionError::HandlerNotDefined
                    },
                    FunctionInvokeApiError::FunctionPanicked { message } => {
                        // panics are usually not recoverable, so we need to re-create context or following
                        // invocations will also fail
                        ctx.needs_recreate.store(true, Ordering::SeqCst);
                        FunctionExecutionError::FunctionPanicked { message }
                    }
                })),
            };

            let mut function_env_mut = ctx.function_env.clone().into_mut(store);
            let result = match function_future_poll(&mut function_env_mut, future_index as u64) {
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
                    FunctionFuturePollApiError::FunctionPanicked { message } => {
                        // panics are usually not recoverable, so we need to re-create context or following
                        // invocations will also fail
                        ctx.needs_recreate.store(true, Ordering::SeqCst);
                        FunctionExecutionError::FunctionPanicked { message }
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
                        compiler_metadata: ctx.function_env.as_ref(store).compiler_metadata.clone(),
                    })))
                }
            };

            // TODO: record points used
            let _points_used = points_before - match get_remaining_points(store, &ctx.instance) {
                MeteringPoints::Remaining(v) => v,
                MeteringPoints::Exhausted => panic!("didn't expect that"),
            };

            drop(store_lock);
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
            ctxs.get(&self.function_id).unwrap().clone()
        };
        ctx.futures_to_drop.lock().unwrap().push_back(*rpc_future_index);
    }
}

pub(crate) struct ExecutionContext {
    pub(crate) instance: Instance,
    pub(crate) store: Arc<Mutex<Store>>,
    pub(crate) function_env: FunctionEnv<ExecutionEnv>,
    // TODO: regular boolean can probably be used here
    needs_recreate: Arc<AtomicBool>,
    futures_to_drop: Arc<Mutex<VecDeque<i64>>>,
    streams_to_drop: Arc<Mutex<VecDeque<i64>>>,
}

impl ExecutionContext {
    pub fn new(
        engine: Arc<Engine>,
        function_id: FunctionId,
        storage: HashMap<String, BoxedStorage>,
        sql: HashMap<String, SqlDatabase>,
        rpc: HashMap<String, RpcBinding>,
        module_code: Vec<u8>,
        allow_fetch: bool,
        allow_log: bool
    ) -> Result<Self, FunctionInvokeError> {
        let (mut store, module, compiler_metadata) = engine.compiler.read().unwrap().compile(&function_id, module_code)
            .map_err(|err| FunctionInvokeError::FailedToCompile(err))?;

        let function_env = FunctionEnv::new(
            &mut store,
            ExecutionEnv::new(engine, function_id, storage, sql, rpc, allow_fetch, allow_log, compiler_metadata)
        );

        let mut import_object = imports! {
            "fx" => {
                "fx_api" => Function::new_typed_with_env(&mut store, &function_env, crate::api::fx_api_handler),
            },
        };

        // some libraries, like leptos, have wbidgen imports, but do not use them. Let's add them here so that module can be linked
        for import in module.imports().into_iter() {
            let module = import.module();
            if module != "fx" {
                match import.ty() {
                    wasmer::ExternType::Function(f) => {
                        import_object.define(module, import.name(), Function::new_with_env(&mut store, &function_env, f, crate::api::unsupported::handle_unsupported));
                    },
                    other => panic!("unexpected import type: {other:?}"),
                }
            }
        }

        let instance = Instance::new(&mut store, &module, &import_object)
            .map_err(|err| FunctionInvokeError::InstantionError(err))?;

        Ok(Self {
            instance,
            store: Arc::new(Mutex::new(store)),
            function_env,
            needs_recreate: Arc::new(AtomicBool::new(false)),
            futures_to_drop: Arc::new(Mutex::new(VecDeque::new())),
            streams_to_drop: Arc::new(Mutex::new(VecDeque::new())),
        })
    }
}

pub(crate) struct ExecutionEnv {
    execution_context: RwLock<Option<Arc<ExecutionContext>>>,
    compiler_metadata: CompilerMetadata,

    pub(crate) futures_waker: Option<std::task::Waker>,

    pub(crate) engine: Arc<Engine>,
    instance: Option<Instance>,
    pub(crate) memory: Option<Memory>,
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
        storage: HashMap<String, BoxedStorage>,
        sql: HashMap<String, SqlDatabase>,
        rpc: HashMap<String, RpcBinding>,
        allow_fetch: bool,
        allow_log: bool,
        compiler_metadata: CompilerMetadata,
    ) -> Self {
        Self {
            execution_context: RwLock::new(None),
            compiler_metadata,
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

    pub fn client_malloc(&self) -> &Function {
        self.instance.as_ref().unwrap().exports.get_function("_fx_malloc").unwrap()
    }
}

/// Error that occured while calling invoke api of the function
#[derive(Error, Debug)]
enum FunctionInvokeApiError {
    /// Handler with this name is not defined in the function
    #[error("handler with this name is not defined")]
    HandlerNotDefined,

    /// Failed to invoke function because it panicked
    #[error("function panicked: {message}")]
    FunctionPanicked {
        message: String
    },
}

fn function_invoke(ctx: &mut FunctionEnvMut<ExecutionEnv>, handler: String, payload: Vec<u8>) -> Result<u64, FunctionInvokeApiError> {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_function_api_call::Builder>();
    let op = request.init_op();
    let mut function_invoke_request = op.init_invoke();
    function_invoke_request.set_method(handler);
    function_invoke_request.set_payload(&payload);

    let response = invoke_fx_api(ctx, message)
        .map_err(|err| match err {
            FunctionApiError::FunctionPanicked { message } => FunctionInvokeApiError::FunctionPanicked { message },
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
    #[error("function panicked: {message}")]
    FunctionPanicked {
        message: String
    },
}

fn function_future_poll(ctx: &mut FunctionEnvMut<ExecutionEnv>, future_id: u64) -> Result<Poll<Vec<u8>>, FunctionFuturePollApiError> {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_function_api_call::Builder>();
    let op = request.init_op();
    let mut future_poll_request = op.init_future_poll();
    future_poll_request.set_future_id(future_id);

    let response = invoke_fx_api(ctx, message)
        .map_err(|err| match err {
            FunctionApiError::FunctionPanicked { message } => FunctionFuturePollApiError::FunctionPanicked { message },
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

fn function_future_drop(ctx: &mut FunctionEnvMut<ExecutionEnv>, future_id: u64) {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_function_api_call::Builder>();
    let op = request.init_op();
    let mut future_drop_request = op.init_future_drop();
    future_drop_request.set_future_id(future_id);

    let _response = invoke_fx_api(ctx, message).unwrap();
}

fn function_stream_poll_next(ctx: &mut FunctionEnvMut<ExecutionEnv>, stream_id: u64) -> Poll<Option<Vec<u8>>> {
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

fn function_stream_drop(ctx: &mut FunctionEnvMut<ExecutionEnv>, stream_id: u64) {
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
    #[error("function panicked: {message:?}")]
    FunctionPanicked {
        message: String,
    },
}

pub(crate) fn invoke_fx_api(
    ctx: &mut FunctionEnvMut<ExecutionEnv>,
    message: capnp::message::Builder<capnp::message::HeapAllocator>
) -> Result<capnp::message::Reader<capnp::serialize::OwnedSegments>, FunctionApiError> {
    let response_ptr = {
        let (data, mut store) = ctx.data_and_store_mut();

        let message_size = capnp::serialize::compute_serialized_size_in_words(&message) * 8;
        let ptr = data.client_malloc().call(&mut store, &[Value::I64(message_size as i64)]).unwrap()[0].i64().unwrap() as usize;

        let memory = data.memory.as_ref().unwrap();

        unsafe {
            capnp::serialize::write_message(
                &mut memory.view(&store).data_unchecked_mut()[ptr..ptr+message_size],
                &message
            ).unwrap();
        }

        data.instance.as_ref().unwrap().exports.get_function("_fx_api").unwrap()
            .call(&mut store, &[Value::I64(ptr as i64), Value::I64(message_size as i64)])
            .map_err(|err| FunctionApiError::FunctionPanicked {
                message: format!("{err:?}"),
            })?[0]
            .i64().unwrap() as usize
    };

    let view = ctx.data().memory.as_ref().unwrap().view(&ctx);
    let header_length = 4;
    let header_bytes = {
        let response_ptr = response_ptr as u64;
        view.copy_range_to_vec(response_ptr..response_ptr+header_length).unwrap()
    };
    let response_length = u32::from_le_bytes(header_bytes.try_into().unwrap());

    let response = {
        let response_ptr = response_ptr as u64;
        let response_length = response_length as u64;
        view.copy_range_to_vec((response_ptr + header_length)..(response_ptr + header_length + response_length)).unwrap()
    };
    let (data, mut store) = ctx.data_and_store_mut();
    data.instance.as_ref().unwrap().exports.get_function("_fx_dealloc").unwrap()
        .call(&mut store, &[Value::I64(response_ptr as i64), Value::I64((header_length as u32 + response_length) as i64)])
        .unwrap();

    Ok(capnp::serialize::read_message(Cursor::new(response), capnp::message::ReaderOptions::default()).unwrap())
}

pub fn read_memory_owned(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) -> Vec<u8> {
    let memory = ctx.data().memory.as_ref().unwrap();
    let view = memory.view(&ctx);
    let addr = addr as u64;
    let len = len as u64;
    view.copy_range_to_vec(addr..addr+len).unwrap()
}

pub fn write_memory_obj<T: Sized>(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, obj: T) {
    write_memory(ctx, addr, unsafe { std::slice::from_raw_parts(&obj as *const T as *const u8, std::mem::size_of_val(&obj)) });
}

pub fn write_memory(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, value: &[u8]) {
    let memory = ctx.data().memory.as_ref().unwrap();
    let view = memory.view(&ctx);
    view.write(addr as u64, value).unwrap();
}

pub fn decode_memory<T: serde::de::DeserializeOwned>(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) -> Result<T, FxRuntimeError> {
    let memory = read_memory_owned(&ctx, addr, len);
    rmp_serde::from_slice(&memory)
        .map_err(|err| FxRuntimeError::SerializationError { reason: format!("failed to decode memory: {err:?}") })
}

fn api_list_functions(mut ctx: FunctionEnvMut<ExecutionEnv>, output_ptr: i64) {
    // TODO: permissions check
    let functions: Vec<_> = ctx.data().engine.execution_contexts.read().unwrap()
        .iter()
        .map(|(function_id, _function)| fx_runtime_common::Function {
            id: function_id.id.clone(),
        })
        .collect();

    let (data, mut store) = ctx.data_and_store_mut();
    let res = rmp_serde::to_vec(&functions).unwrap();
    let len = res.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &res);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
}

#[repr(C)]
pub(crate) struct PtrWithLen {
    pub ptr: i64,
    pub len: i64,
}

pub(crate) struct RpcBinding {}

pub struct FunctionInvocationEvent {
    pub compiler_metadata: CompilerMetadata,
}
