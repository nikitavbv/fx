use {
    std::{
        sync::{Arc, Mutex, RwLock, atomic::{AtomicBool, AtomicU64, Ordering}},
        collections::{HashMap, VecDeque},
        ops::DerefMut,
        task::{self, Poll},
        time::{SystemTime, UNIX_EPOCH, Instant},
        io::Cursor,
        cell::RefCell,
    },
    tracing::{error, info},
    wasmtime::{AsContext, AsContextMut, InstancePre, Instance, Store},
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
    crate::{
        common::{LogMessageEvent, LogSource},
        runtime::{
            kv::{KVStorage, NamespacedStorage, EmptyStorage, BoxedStorage, FsStorage, StorageError},
            error::{FxRuntimeError, FunctionInvokeError, FunctionInvokeInternalRuntimeError},
            sql::{self, SqlDatabase},
            metrics::Metrics,
            definition::{DefinitionProvider, FunctionDefinition, SqlStorageDefinition, DefinitionError},
            logs::{self, Logger, BoxLogger, StdoutLogger},
        },
        introspection::IntrospectionState,
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
    pub async fn invoke_service<S: serde::de::DeserializeOwned>(&self, service: &FunctionId, request: FunctionRequest) -> Result<(S, FunctionInvocationEvent), FunctionInvokeAndExecuteError> {
        self.engine.invoke_service(self.engine.clone(), service, request).await
    }

    pub fn invoke_service_raw(&self, service: &FunctionId, request: FunctionRequest) -> Result<FunctionRuntimeFuture, FunctionInvokeError> {
        self.engine.invoke_service_raw(self.engine.clone(), service.clone(), request)
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
    pub introspection: Arc<IntrospectionState>,

    deployed_functions: RwLock<HashMap<FunctionId, ArcSwap<ExecutionContextId>>>,
    execution_context_id_counter: AtomicU64,
    pub execution_contexts: RwLock<HashMap<ExecutionContextId, Arc<ExecutionContext>>>,
    definition_provider: RwLock<DefinitionProvider>,

    // internal storage where .wasm is loaded from:
    module_code_storage: RwLock<BoxedStorage>,

    logger: RwLock<BoxLogger>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            wasmtime: wasmtime::Engine::default(),

            metrics: Metrics::new(),
            introspection: Arc::new(IntrospectionState::new()),

            deployed_functions: RwLock::new(HashMap::new()),
            execution_context_id_counter: AtomicU64::new(0),
            execution_contexts: RwLock::new(HashMap::new()),
            definition_provider: RwLock::new(DefinitionProvider::new(BoxedStorage::new(EmptyStorage))),

            module_code_storage: RwLock::new(BoxedStorage::new(NamespacedStorage::new(b"services/", EmptyStorage))),

            logger: RwLock::new(BoxLogger::new(StdoutLogger::new())),
        }
    }

    pub async fn invoke_service<S: serde::de::DeserializeOwned>(&self, engine: Arc<Engine>, service: &FunctionId, request: FunctionRequest) -> Result<(S, FunctionInvocationEvent), FunctionInvokeAndExecuteError> {
        let (response, event) = self.invoke_service_raw(engine, service.clone(), request)
            .map_err(|err| match err {
                FunctionInvokeError::RuntimeError(err) => {
                    error!("runtime error when invoking function: {err:?}");
                    FunctionInvokeAndExecuteError::RuntimeError
                },
                FunctionInvokeError::DefinitionMissing(err) => FunctionInvokeAndExecuteError::DefinitionMissing(err),
                FunctionInvokeError::CodeFailedToLoad(err) => FunctionInvokeAndExecuteError::CodeFailedToLoad(err),
                FunctionInvokeError::CodeNotFound => FunctionInvokeAndExecuteError::CodeNotFound,
                FunctionInvokeError::FailedToCompile(err) => FunctionInvokeAndExecuteError::FailedToCompile(err),
                FunctionInvokeError::FailedToInstantiate(err) => FunctionInvokeAndExecuteError::FailedToInstantiate(err),
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

    pub fn invoke_service_raw(&self, engine: Arc<Engine>, function_id: FunctionId, request: FunctionRequest) -> Result<FunctionRuntimeFuture, FunctionInvokeError> {
        // TODO: will be able to remove this once switched to new server?
        let context_id = self.resolve_context_id_for_function(&function_id);

        {
            let execution_contexts = match self.execution_contexts.read() {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to lock execution contexts: {err:?}");
                    return Err(FunctionInvokeError::from(
                        FunctionInvokeInternalRuntimeError::ExecutionContextsFailedToLock { reason: err.to_string() }
                    ));
                }
            };

            let context = execution_contexts.get(&context_id)
                .ok_or(FunctionInvokeError::CodeNotFound)?;
            if context.needs_recreate.load(Ordering::SeqCst) {
                context.reset(engine.clone())?;
            }
        }

        Ok(self.run_service(engine, function_id.clone(), (*context_id).clone(), request))
    }

    pub fn reload(&self, function_id: &FunctionId) {
        let context_id = self.resolve_context_id_for_function(function_id);
        if let Some(execution_context) = self.execution_contexts.read().unwrap().get(&context_id) {
            println!("reloading {}", function_id.id);
            execution_context.needs_recreate.store(true, Ordering::SeqCst);
        }
    }

    fn run_service(&self, engine: Arc<Engine>, function_id: FunctionId, execution_context_id: ExecutionContextId, request: FunctionRequest) -> FunctionRuntimeFuture {
        FunctionRuntimeFuture {
            engine,
            function_id,
            execution_context_id,
            request,
            rpc_future_index: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn resolve_context_id_for_function(&self, function_id: &FunctionId) -> Arc<ExecutionContextId> {
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
        kv: HashMap<String, BoxedStorage>,
        logger: Option<BoxLogger>,
    ) -> Result<ExecutionContextId, FunctionInvokeError> {
        let execution_context_id = ExecutionContextId::new(self.execution_context_id_counter.fetch_add(1, Ordering::SeqCst));

        let execution_context = ExecutionContext::new(
            engine,
            function_id,
            execution_context_id.clone(),
            kv,
            sql,
            rpc,
            function_module,
            true,
            true,
            logger,
        )?;
        self.execution_contexts.write().unwrap().insert(execution_context_id.clone(), Arc::new(execution_context));

        Ok(execution_context_id)
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
            None,
        );

        if let Some(memory_usage) = memory_tracker.report_total() {
            engine.metrics.function_execution_context_init_memory_usage.with_label_values(&[function_id.as_string()]).set(memory_usage as i64);
        }

        execution_context
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

    /// Function cannot be invoked if failed to create instance (failed to link, etc.)
    #[error("failed to instantiate: {0:?}")]
    FailedToInstantiate(wasmtime::Error),
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
    request: FunctionRequest,
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
        unimplemented!("deprecated")
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
    instance_template: InstancePre<ExecutionEnv>,
    pub(crate) instance: RefCell<Instance>,
    pub(crate) store: Arc<Mutex<Store<ExecutionEnv>>>,
    // pub(crate) function_env: FunctionEnv<ExecutionEnv>,
    // TODO: regular boolean can probably be used here
    needs_recreate: Arc<AtomicBool>,
    futures_to_drop: Arc<Mutex<VecDeque<i64>>>,
    streams_to_drop: Arc<Mutex<VecDeque<i64>>>,
}

// TODO: hack. Runtime is single-threaded anyway. Need to figure out if I can remove requirement for fields to be Sync
unsafe impl Sync for ExecutionContext {}

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
        allow_log: bool,
        logger: Option<BoxLogger>,
    ) -> Result<Self, FunctionInvokeError> {
        unimplemented!("deprecated")
    }

    fn reset(&self, engine: Arc<Engine>) -> Result<(), FunctionInvokeError> {
        let mut store = self.store.lock().unwrap();
        let env = store.data_mut().reset();
        *store = wasmtime::Store::new(&engine.wasmtime, env);

        *self.instance.borrow_mut() = self.instance_template.instantiate(store.as_context_mut())
            .map_err(|err| FunctionInvokeError::FailedToInstantiate(err))?;
        drop(store);

        self.needs_recreate.store(false, Ordering::SeqCst);
        self.futures_to_drop.lock().unwrap().clear();
        self.streams_to_drop.lock().unwrap().clear();

        Ok(())
    }
}

pub(crate) struct ExecutionEnv {
    execution_context: RwLock<Option<Arc<ExecutionContext>>>,
    pub(crate) execution_context_id: ExecutionContextId,

    pub(crate) futures_waker: Option<std::task::Waker>,

    pub(crate) engine: Arc<Engine>,
    pub(crate) instance: Option<RefCell<Instance>>,
    pub(crate) memory: Option<wasmtime::Memory>,

    // TODO: these fields are not needed anymore?
    pub(crate) execution_error: Option<Vec<u8>>,
    pub(crate) rpc_response: Option<Vec<u8>>,

    pub(crate) function_id: FunctionId,

    pub(crate) storage: HashMap<String, BoxedStorage>,
    pub(crate) sql: HashMap<String, SqlDatabase>,
    pub(crate) rpc: HashMap<String, RpcBinding>,

    // TODO: replace with a better permissions model
    allow_fetch: bool,
    allow_log: bool,

    pub(crate) fetch_client: reqwest::Client,
    pub(crate) logger: Option<BoxLogger>,
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
        logger: Option<BoxLogger>,
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
            logger,
        }
    }

    fn reset(&mut self) -> ExecutionEnv {
        let execution_context = std::mem::replace(&mut self.execution_context, RwLock::new(None));
        let futures_waker = std::mem::replace(&mut self.futures_waker, None);

        let storage = std::mem::replace(&mut self.storage, HashMap::new());
        let sql = std::mem::replace(&mut self.sql, HashMap::new());
        let rpc = std::mem::replace(&mut self.rpc, HashMap::new());
        let logger = std::mem::replace(&mut self.logger, None);

        Self {
            execution_context,
            execution_context_id: self.execution_context_id.clone(),

            futures_waker,
            engine: self.engine.clone(),
            instance: None,
            memory: None,

            execution_error: None,
            rpc_response: None,

            function_id: self.function_id.clone(),

            storage,
            sql,
            rpc,

            allow_fetch: true,
            allow_log: true,

            fetch_client: reqwest::Client::new(),
            logger,
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

#[derive(Clone)]
pub(crate) enum FunctionRequest {
    Http,
    Cron {
        task_name: String,
    },
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

/// Error that occurred while communicating with function api
#[derive(Error, Debug)]
enum FunctionApiError {
    /// Api request failed because function code panicked
    #[error("function panicked")]
    FunctionPanicked,
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
