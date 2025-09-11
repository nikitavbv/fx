use {
    std::{
        sync::{Arc, Mutex, RwLock, atomic::{AtomicBool, Ordering}},
        collections::{HashMap, VecDeque},
        ops::DerefMut,
        task::{self, Poll},
        time::{SystemTime, UNIX_EPOCH, Instant},
    },
    tracing::error,
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
    fx_runtime_common::LogMessageEvent,
    crate::{
        kv::{KVStorage, NamespacedStorage, EmptyStorage, BoxedStorage, FsStorage},
        error::FxCloudError,
        compatibility,
        sql::{self, SqlDatabase},
        compiler::{Compiler, BoxedCompiler, SimpleCompiler},
        futures::FuturesPool,
        streams::StreamsPool,
        metrics::Metrics,
        definition::{DefinitionProvider, FunctionDefinition, SqlStorageDefinition},
        logs::{self, Logger, BoxLogger, StdoutLogger},
    },
};

#[derive(Clone)]
pub struct FxCloud {
    pub(crate) engine: Arc<Engine>,
}

impl FxCloud {
    pub fn new() -> Self {
        Self {
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
    pub async fn invoke_service<T: serde::ser::Serialize, S: serde::de::DeserializeOwned>(&self, service: &FunctionId, function_name: &str, argument: T) -> Result<S, FxCloudError> {
        self.engine.invoke_service(self.engine.clone(), service, function_name, argument).await
    }

    pub fn invoke_service_raw(&self, service: &FunctionId, function_name: &str, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FxCloudError> {
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
    pub(crate) metrics: Metrics,

    compiler: RwLock<BoxedCompiler>,

    pub(crate) execution_contexts: RwLock<HashMap<FunctionId, Arc<ExecutionContext>>>,
    definition_provider: RwLock<DefinitionProvider>,

    // internal storage where .wasm is loaded from:
    module_code_storage: RwLock<BoxedStorage>,

    pub(crate) futures_pool: FuturesPool,
    pub(crate) streams_pool: StreamsPool,

    logger: RwLock<BoxLogger>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            metrics: Metrics::new(),

            compiler: RwLock::new(BoxedCompiler::new(SimpleCompiler::new())),

            execution_contexts: RwLock::new(HashMap::new()),
            definition_provider: RwLock::new(DefinitionProvider::new(BoxedStorage::new(EmptyStorage))),

            module_code_storage: RwLock::new(BoxedStorage::new(NamespacedStorage::new(b"services/", EmptyStorage))),

            futures_pool: FuturesPool::new(),
            streams_pool: StreamsPool::new(),

            logger: RwLock::new(BoxLogger::new(StdoutLogger::new())),
        }
    }

    pub async fn invoke_service<T: serde::ser::Serialize, S: serde::de::DeserializeOwned>(&self, engine: Arc<Engine>, service: &FunctionId, function_name: &str, argument: T) -> Result<S, FxCloudError> {
        let argument = rmp_serde::to_vec(&argument).unwrap();
        let response = self.invoke_service_raw(engine, service.clone(), function_name.to_owned(), argument)?.await?;
        Ok(rmp_serde::from_slice(&response).unwrap())
    }

    pub fn invoke_service_raw(&self, engine: Arc<Engine>, service_id: FunctionId, function_name: String, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FxCloudError> {
        let need_to_create_context = {
            let execution_contexts = match self.execution_contexts.read() {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to lock execution contexts: {err:?}");
                    return Err(FxCloudError::ExecutionContextRuntimeError { reason: format!("failed to lock execution contexts: {err:?}") });
                }
            };
            if let Some(context) = execution_contexts.get(&service_id) {
                context.needs_recreate.load(Ordering::SeqCst)
            } else {
                true
            }
        };

        if need_to_create_context {
            // need to create execution context first
            let mut execution_contexts = self.execution_contexts.write().unwrap();
            let ctx = execution_contexts.get(&service_id);
            if ctx.map(|v| v.needs_recreate.load(Ordering::SeqCst)).unwrap_or(true) {
                let definition = self.definition_provider.read().unwrap().definition_for_function(&service_id)
                    .map_err(|err| FxCloudError::DefinitionError { reason: err.to_string() })?;
                execution_contexts.insert(service_id.clone(), Arc::new(self.create_execution_context(engine.clone(), &service_id, definition)?));
            }
        }

        Ok(self.run_service(engine, service_id.clone(), &function_name, argument))
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

    fn create_execution_context(&self, engine: Arc<Engine>, service_id: &FunctionId, definition: FunctionDefinition) -> Result<ExecutionContext, FxCloudError> {
        let memory_tracker_total = crate::profiling::init_memory_tracker();

        let module_code = self.module_code_storage.read().unwrap().get(service_id.id.as_bytes())?;
        let module_code = match module_code {
            Some(v) => v,
            None => return Err(FxCloudError::ModuleCodeNotFound),
        };

        let mut kv = HashMap::new();
        for kv_definition in definition.kv {
            kv.insert(
                kv_definition.id,
                BoxedStorage::new(FsStorage::new(kv_definition.path.into())?),
            );
        }

        let mut sql = HashMap::new();
        for sql_definition in definition.sql {
            sql.insert(
                sql_definition.id,
                match sql_definition.storage {
                    SqlStorageDefinition::InMemory => SqlDatabase::in_memory().unwrap(), // TODO: function crashes, all data is lost
                    SqlStorageDefinition::Path(path) => SqlDatabase::new(path)
                        .map_err(|err| FxCloudError::ExecutionContextInitError {
                            reason: format!("failed to init SqlDatabase: {err:?}"),
                        })?,
                },
            );
        }

        let memory_tracker_context_new = crate::profiling::init_memory_tracker();
        let execution_context = ExecutionContext::new(
            engine.clone(),
            service_id.clone(),
            kv,
            sql,
            module_code,
            true, // TODO: permissions
            true, // TODO: permissions
        );

        tracing::info!("memory used to create execution context: {:?}/{:?}", memory_tracker_total.report_total(), memory_tracker_context_new.report_total());

        execution_context
    }

    pub(crate) fn stream_poll_next(&self, function_id: &FunctionId, index: i64) -> Poll<Option<Result<Vec<u8>, FxCloudError>>> {
        let ctxs = self.execution_contexts.read().unwrap();
        let ctx = ctxs.get(function_id).unwrap();
        let mut store_lock = ctx.store.lock().unwrap();
        let store = store_lock.deref_mut();

        let function_stream_next = ctx.instance.exports.get_function("_fx_stream_next").unwrap();
        // TODO: measure points
        let poll_next = function_stream_next.call(store, &[wasmer::Value::I64(index)]).unwrap()[0].unwrap_i64();
        match poll_next {
            0 => Poll::Pending,
            1 => {
                let response = ctx.function_env.as_ref(store).rpc_response.as_ref().unwrap().clone();
                Poll::Ready(Some(Ok(response)))
            },
            2 => Poll::Ready(None),
            other => Poll::Ready(Some(Err(FxCloudError::StreamingError {
                reason: format!("unexpected repsonse code from _fx_stream_next: {other:?}"),
            }))),
        }
    }

    pub(crate) fn stream_drop(&self, function_id: &FunctionId, index: i64) {
        let ctxs = self.execution_contexts.read().unwrap();
        let ctx = ctxs.get(function_id).unwrap();
        let mut store_lock = ctx.store.lock().unwrap();
        let store = store_lock.deref_mut();

        let function_stream_drop = ctx.instance.exports.get_function("_fx_stream_drop").unwrap();
        function_stream_drop.call(store, &[wasmer::Value::I64(index)]).unwrap();
    }

    pub(crate) fn log(&self, message: LogMessageEvent) {
        self.logger.read().unwrap().log(message);
    }
}

pub struct FunctionRuntimeFuture {
    engine: Arc<Engine>,
    function_id: FunctionId,
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
    type Output = Result<Vec<u8>, FxCloudError>;
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
                return std::task::Poll::Ready(Err(FxCloudError::ExecutionContextRuntimeError {
                    reason: format!("failed to lock ctx.store when polling FunctionRuntimeFuture."),
                }));
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
            match ctx.instance.exports.get_function("_fx_future_drop") {
                Ok(function_drop) => {
                    while let Some(future_to_drop) = futures_to_drop.pop_front() {
                        if let Err(err) = function_drop.call(store, &[Value::I64(future_to_drop)]) {
                            error!("failed to call _fx_future_drop: {err:?}");
                        };
                    }
                },
                Err(err) => {
                    error!("_fx_future_drop not available in: {:?}, {err:?}", self.function_id.id);
                }
            }
        }

        // poll this future
        let function_poll = ctx.instance.exports.get_function("_fx_future_poll").unwrap();

        if let Some(rpc_future_index_value) = rpc_future_index.as_ref().clone() {
            // TODO: measure points
            let poll_is_ready = function_poll.call(store, &[Value::I64(*rpc_future_index_value)])
                .map_err(|err| FxCloudError::ServiceInternalError { reason: format!("failed when polling future: {err:?}") })?[0]
                .unwrap_i64();
            let result = if poll_is_ready == 1 {
                let response = ctx.function_env.as_ref(store).rpc_response.as_ref().unwrap().clone();
                *rpc_future_index = None;
                drop(store_lock);
                self.record_function_invocation();
                std::task::Poll::Ready(Ok(response))
            } else {
                std::task::Poll::Pending
            };

            result
        } else {
            ctx.function_env.as_mut(store).futures_waker = Some(cx.waker().clone());

            let points_before = u64::MAX;
            set_remaining_points(store, &ctx.instance, points_before);

            let memory = ctx.instance.exports.get_memory("memory").unwrap();
            ctx.function_env.as_mut(store).instance = Some(ctx.instance.clone());
            ctx.function_env.as_mut(store).memory = Some(memory.clone());

            let client_malloc = ctx.instance.exports.get_function("_fx_malloc").unwrap();
            let target_addr = client_malloc.call(store, &[Value::I64(argument.len() as i64)]).unwrap()[0].unwrap_i64() as u64;
            memory.view(store).write(target_addr, &argument).unwrap();

            let function = ctx.instance.exports.get_function(&format!("_fx_rpc_{function_name}"))
                .map_err(|err| match err {
                   ExportError::Missing(_) => FxCloudError::RpcHandlerNotDefined,
                   ExportError::IncompatibleType => FxCloudError::RpcHandlerIncompatibleType,
                })?;

            ctx.function_env.as_mut(store).execution_error = None;

            // TODO: errors like this should be reported to some data stream
            let future_index = function.call(store, &[Value::I64(target_addr as i64), Value::I64(argument.len() as i64)])
                .map_err(|err| FxCloudError::ServiceInternalError { reason: format!("rpc call failed: {err:?}") });

            if let Some(execution_error) = ctx.function_env.as_ref(store).execution_error.as_ref() {
                let execution_error: FxExecutionError = rmp_serde::from_slice(execution_error.as_slice()).unwrap();
                ctx.needs_recreate.store(true, Ordering::SeqCst);
                return Poll::Ready(Err(FxCloudError::ServiceExecutionError { error: execution_error }));
            }

            let future_index = match future_index {
                Ok(v) => v[0].unwrap_i64(),
                Err(err) => {
                    ctx.needs_recreate.store(true, Ordering::SeqCst);
                    return std::task::Poll::Ready(Err(err));
                }
            };

            let poll_is_ready = function_poll.call(store, &[Value::I64(future_index as i64)])
                .map_err(|err| FxCloudError::ServiceInternalError { reason: format!("rpc call failed: {err:?}") });
            let poll_is_ready = match poll_is_ready {
                Ok(v) => v[0].unwrap_i64(),
                Err(err) => {
                    ctx.needs_recreate.store(true, Ordering::SeqCst);
                    return std::task::Poll::Ready(Err(err));
                }
            };
            let result = if poll_is_ready == 1 {
                let response = ctx.function_env.as_ref(store).rpc_response.as_ref().unwrap().clone();
                std::task::Poll::Ready(Ok(response))
            } else {
                *rpc_future_index = Some(future_index);
                std::task::Poll::Pending
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
    needs_recreate: Arc<AtomicBool>,
    futures_to_drop: Arc<Mutex<VecDeque<i64>>>,
}

impl ExecutionContext {
    pub fn new(
        engine: Arc<Engine>,
        service_id: FunctionId,
        storage: HashMap<String, BoxedStorage>,
        sql: HashMap<String, SqlDatabase>,
        module_code: Vec<u8>,
        allow_fetch: bool,
        allow_log: bool
    ) -> Result<Self, FxCloudError> {
        let mut compiler_config = wasmer_compiler_llvm::LLVM::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, ops_cost_function)));

        let mut store = Store::new(EngineBuilder::new(compiler_config));

        let memory_tracker = crate::profiling::init_memory_tracker();
        let module = engine.compiler.read().unwrap().compile(&store, module_code)
            .map_err(|err| FxCloudError::CompilationError { reason: err.to_string() })?;
        tracing::info!("memory used by compiler: {:?}", memory_tracker.report_total());

        let function_env = FunctionEnv::new(
            &mut store,
            ExecutionEnv::new(engine, service_id.clone(), storage, sql, allow_fetch, allow_log)
        );

        let mut import_object = imports! {
            "fx" => {
                "fx_api" => Function::new_typed_with_env(&mut store, &function_env, api_call_handler),
                "rpc" => Function::new_typed_with_env(&mut store, &function_env, api_rpc),
                "send_rpc_response" => Function::new_typed_with_env(&mut store, &function_env, api_send_rpc_response),
                "send_error" => Function::new_typed_with_env(&mut store, &function_env, api_send_error),
                "kv_get" => Function::new_typed_with_env(&mut store, &function_env, api_kv_get),
                "kv_set" => Function::new_typed_with_env(&mut store, &function_env, api_kv_set),
                "sql_exec" => Function::new_typed_with_env(&mut store, &function_env, api_sql_exec),
                "sql_batch" => Function::new_typed_with_env(&mut store, &function_env, api_sql_batch),
                "sql_migrate" => Function::new_typed_with_env(&mut store, &function_env, api_sql_migrate),
                "queue_push" => Function::new_typed_with_env(&mut store, &function_env, api_queue_push),
                "log" => Function::new_typed_with_env(&mut store, &function_env, api_log),
                "metrics_counter_increment" => Function::new_typed_with_env(&mut store, &function_env, api_metrics_counter_increment),
                "fetch" => Function::new_typed_with_env(&mut store, &function_env, api_fetch),
                "sleep" => Function::new_typed_with_env(&mut store, &function_env, api_sleep),
                "random" => Function::new_typed_with_env(&mut store, &function_env, api_random),
                "time" => Function::new_typed_with_env(&mut store, &function_env, api_time),
                "future_poll" => Function::new_typed_with_env(&mut store, &function_env, api_future_poll),
                "future_drop" => Function::new_typed_with_env(&mut store, &function_env, api_future_drop),
                "stream_export" => Function::new_typed_with_env(&mut store, &function_env, api_stream_export),
                "stream_poll_next" => Function::new_typed_with_env(&mut store, &function_env, api_stream_poll_next),
            },
            "fx_cloud" => {
                "list_functions" => Function::new_typed_with_env(&mut store, &function_env, api_list_functions),
            }
        };

        // some libraries, like leptos, have wbidgen imports, but do not use them. Let's add them here so that module can be linked
        for import in module.imports().into_iter() {
            let module = import.module();
            if module != "fx" && module != "fx_cloud" {
                match import.ty() {
                    wasmer::ExternType::Function(f) => {
                        import_object.define(module, import.name(), Function::new_with_env(&mut store, &function_env, f, compatibility::api_unsupported));
                    },
                    other => panic!("unexpected import type: {other:?}"),
                }
            }
        }

        let memory_tracker = crate::profiling::init_memory_tracker();
        let instance = Instance::new(&mut store, &module, &import_object)
            .map_err(|err| FxCloudError::CompilationError { reason: format!("failed to create wasm instance: {err:?}") })?;
        tracing::info!("memory used by Instance::new: {:?}", memory_tracker.report_total());

        for (export_name, ext) in instance.exports.iter() {
            match ext {
                wasmer::Extern::Memory(memory) => {
                    tracing::info!("memory: {export_name} {}", memory.size(&mut store).bytes().0);
                },
                _ => {},
            }
        }

        Ok(Self {
            instance,
            store: Arc::new(Mutex::new(store)),
            function_env,
            needs_recreate: Arc::new(AtomicBool::new(false)),
            futures_to_drop: Arc::new(Mutex::new(VecDeque::new())),
        })
    }
}

fn ops_cost_function(_: &Operator) -> u64 { 1 }

pub(crate) struct ExecutionEnv {
    execution_context: RwLock<Option<Arc<ExecutionContext>>>,

    futures_waker: Option<std::task::Waker>,

    engine: Arc<Engine>,
    instance: Option<Instance>,
    pub(crate) memory: Option<Memory>,
    execution_error: Option<Vec<u8>>,
    pub(crate) rpc_response: Option<Vec<u8>>,

    service_id: FunctionId,

    storage: HashMap<String, BoxedStorage>,
    sql: HashMap<String, SqlDatabase>,

    allow_fetch: bool,
    allow_log: bool,

    fetch_client: reqwest::Client,
}

impl ExecutionEnv {
    pub fn new(
        engine: Arc<Engine>,
        service_id: FunctionId,
        storage: HashMap<String, BoxedStorage>,
        sql: HashMap<String, SqlDatabase>,
        allow_fetch: bool,
        allow_log: bool
    ) -> Self {
        Self {
            execution_context: RwLock::new(None),
            futures_waker: None,
            engine,
            instance: None,
            memory: None,
            execution_error: None,
            rpc_response: None,
            service_id,
            storage,
            sql,
            allow_fetch,
            allow_log,
            fetch_client: reqwest::Client::new(),
        }
    }

    fn client_malloc(&self) -> &Function {
        self.instance.as_ref().unwrap().exports.get_function("_fx_malloc").unwrap()
    }
}

fn read_memory_owned(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) -> Vec<u8> {
    let memory = ctx.data().memory.as_ref().unwrap();
    let view = memory.view(&ctx);
    let addr = addr as u64;
    let len = len as u64;
    view.copy_range_to_vec(addr..addr+len).unwrap()
}

fn write_memory_obj<T: Sized>(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, obj: T) {
    write_memory(ctx, addr, unsafe { std::slice::from_raw_parts(&obj as *const T as *const u8, std::mem::size_of_val(&obj)) });
}

fn write_memory(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, value: &[u8]) {
    let memory = ctx.data().memory.as_ref().unwrap();
    let view = memory.view(&ctx);
    view.write(addr as u64, value).unwrap();
}

fn decode_memory<T: serde::de::DeserializeOwned>(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) -> Result<T, FxCloudError> {
    let memory = read_memory_owned(&ctx, addr, len);
    rmp_serde::from_slice(&memory)
        .map_err(|err| FxCloudError::SerializationError { reason: format!("failed to decode memory: {err:?}") })
}

fn api_call_handler(ctx: FunctionEnvMut<ExecutionEnv>, request_ptr: i64, request_len: i64, output_ptr: i64) -> i64 {
    let request: fx_common::api::FxApiRequest = decode_memory(&ctx, request_ptr, request_len).unwrap();
    0
}

fn api_rpc(
    ctx: FunctionEnvMut<ExecutionEnv>,
    service_name_ptr: i64,
    service_name_len: i64,
    function_name_ptr: i64,
    function_name_len: i64,
    arg_ptr: i64,
    arg_len: i64,
) -> i64 {
    let service_id = FunctionId::new(String::from_utf8(read_memory_owned(&ctx, service_name_ptr, service_name_len)).unwrap());
    let function_name = String::from_utf8(read_memory_owned(&ctx, function_name_ptr, function_name_len)).unwrap();
    let argument = read_memory_owned(&ctx, arg_ptr, arg_len);

    let engine = ctx.data().engine.clone();
    let response_future = match engine.clone().invoke_service_raw(engine.clone(), service_id, function_name, argument) {
        Ok(response_future) => response_future.map(|v| v.map_err(|err| FxFutureError::RpcError {
            reason: err.to_string(),
        })).boxed(),
        Err(err) => std::future::ready(Err(FxFutureError::RpcError { reason: err.to_string() })).boxed(),
    };
    let response_future = match ctx.data().engine.futures_pool.push(response_future.boxed()) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to push future to futures arena: {err:?}");
            // todo: write error object
            return -1;
        }
    };

    response_future.0 as i64
}

fn api_send_rpc_response(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    ctx.data_mut().rpc_response = Some(read_memory_owned(&ctx, addr, len));
}

fn api_send_error(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    ctx.data_mut().execution_error = Some(read_memory_owned(&ctx, addr, len));
}

fn api_kv_get(mut ctx: FunctionEnvMut<ExecutionEnv>, binding_addr: i64, binding_len: i64, k_addr: i64, k_len: i64, output_ptr: i64) -> i64 {
    let binding = String::from_utf8(read_memory_owned(&ctx, binding_addr, binding_len)).unwrap();
    let storage = match ctx.data().storage.get(&binding) {
        Some(v) => v,
        None => return 1,
    };

    let key = read_memory_owned(&ctx, k_addr, k_len);
    let value = storage.get(&key).unwrap();
    let value = match value {
        Some(v) => v,
        None => return 2,
    };

    let (data, mut store) = ctx.data_and_store_mut();

    let len = value.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &value);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });

    0
}

fn api_kv_set(ctx: FunctionEnvMut<ExecutionEnv>, binding_addr: i64, binding_len: i64, k_addr: i64, k_len: i64, v_addr: i64, v_len: i64) -> i64 {
    let binding = String::from_utf8(read_memory_owned(&ctx, binding_addr, binding_len)).unwrap();
    let storage = match ctx.data().storage.get(&binding) {
        Some(v) => v,
        None => return 1,
    };

    let key = read_memory_owned(&ctx, k_addr, k_len);
    let value = read_memory_owned(&ctx, v_addr, v_len);
    // TODO: report errors to calling service
    storage.set(&key, &value).unwrap();

    0
}

fn api_sql_exec(mut ctx: FunctionEnvMut<ExecutionEnv>, query_addr: i64, query_len: i64, output_ptr: i64) {
    let result = decode_memory(&ctx, query_addr, query_len)
        .map_err(|err| FxSqlError::SerializationError { reason: format!("failed to decode request: {err:?}") })
        .and_then(|request: DatabaseSqlQuery| {
            let mut query = sql::Query::new(request.query.stmt);
            for param in request.query.params {
                query = query.with_param(match param {
                    SqlValue::Null => sql::Value::Null,
                    SqlValue::Integer(v) => sql::Value::Integer(v),
                    SqlValue::Real(v) => sql::Value::Real(v),
                    SqlValue::Text(v) => sql::Value::Text(v),
                    SqlValue::Blob(v) => sql::Value::Blob(v),
                });
            }

            ctx.data().sql.get(&request.database)
                .as_ref()
                .ok_or(FxSqlError::BindingNotExists)
                .and_then(|database| database.exec(query).map_err(|err| FxSqlError::QueryFailed { reason: err.to_string() }))
                .map(|result| SqlResult {
                    rows: result.rows.into_iter()
                        .map(|row| SqlResultRow {
                            columns: row.columns.into_iter()
                                .map(|value| match value {
                                    sql::Value::Null => SqlValue::Null,
                                    sql::Value::Integer(v) => SqlValue::Integer(v),
                                    sql::Value::Real(v) => SqlValue::Real(v),
                                    sql::Value::Text(v) => SqlValue::Text(v),
                                    sql::Value::Blob(v) => SqlValue::Blob(v),
                                })
                                .collect(),
                        })
                        .collect(),
                })
        });
    let result = rmp_serde::to_vec(&result).unwrap();

    let (data, mut store) = ctx.data_and_store_mut();

    let len = result.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &result);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
}

fn api_sql_batch(mut ctx: FunctionEnvMut<ExecutionEnv>, query_addr: i64, query_len: i64, output_ptr: i64) {
    let data = ctx.data();
    let result: Result<(), FxSqlError> = decode_memory(&ctx, query_addr, query_len)
        .map(|request: DatabaseSqlBatchQuery| {
            let queries = request.queries.into_iter()
                .map(|request_query| {
                    let mut query = sql::Query::new(request_query.stmt);
                    for param in request_query.params {
                        query = query.with_param(match param {
                             SqlValue::Null => sql::Value::Null,
                            SqlValue::Integer(v) => sql::Value::Integer(v),
                            SqlValue::Real(v) => sql::Value::Real(v),
                            SqlValue::Text(v) => sql::Value::Text(v),
                            SqlValue::Blob(v) => sql::Value::Blob(v),
                        });
                    }
                    query
                })
                .collect::<Vec<_>>();

            // TODO: report errors to calling service
            data.sql.get(&request.database).as_ref().unwrap().batch(queries).unwrap();
        })
        .map_err(|err| FxSqlError::SerializationError { reason: format!("failed to decode request: {err:?}") });

    let (data, mut store) = ctx.data_and_store_mut();
    let result = rmp_serde::to_vec(&result).unwrap();
    let len = result.len() as i64;

    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &result);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
}

fn api_sql_migrate(mut ctx: FunctionEnvMut<ExecutionEnv>, migration_addr: i64, migration_len: i64, output_ptr: i64) {
    let result = decode_memory(&ctx, migration_addr, migration_len)
        .map_err(|err| FxSqlError::SerializationError { reason: format!("failed to decode migrations request: {err:?}") })
        .and_then(|migrations: SqlMigrations| {
            ctx.data().sql.get(&migrations.database).as_ref().unwrap().migrate(migrations)
                .map_err(|err| fx_common::FxSqlError::MigrationFailed {
                    reason: err.to_string(),
                })
        });

    let result = rmp_serde::to_vec(&result).unwrap();

    let (data, mut store) = ctx.data_and_store_mut();
    let len = result.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &result);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
}

fn api_queue_push(ctx: FunctionEnvMut<ExecutionEnv>, queue_addr: i64, queue_len: i64, argument_addr: i64, argument_len: i64) {
    // TODO: queues need to come back in a different form
    let _queue = String::from_utf8(read_memory_owned(&ctx, queue_addr, queue_len)).unwrap();
    let _argument = read_memory_owned(&ctx, argument_addr, argument_len);
    let _engine = ctx.data().engine.clone();
    // tokio::task::spawn(async move { engine.push_to_queue_raw(queue, argument).await });
}

fn api_log(ctx: FunctionEnvMut<ExecutionEnv>, msg_addr: i64, msg_len: i64) {
    if !ctx.data().allow_log {
        // TODO: record a metric somewhere
        return;
    }
    let msg: LogMessage = match decode_memory(&ctx, msg_addr, msg_len) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to decode memory for log message: {err:?}");
            return;
        }
    };

    let level = match msg.level {
        fx_common::LogLevel::Trace => logs::LogLevel::Trace,
        fx_common::LogLevel::Debug => logs::LogLevel::Debug,
        fx_common::LogLevel::Info => logs::LogLevel::Info,
        fx_common::LogLevel::Warn => logs::LogLevel::Warn,
        fx_common::LogLevel::Error => logs::LogLevel::Error,
    };

    let ctx_data = ctx.data();
    ctx_data.engine.log(crate::logs::LogMessage::new(
        crate::logs::LogSource::function(&ctx_data.service_id),
        level,
        msg.fields,
    ).into());
}

fn api_metrics_counter_increment(ctx: FunctionEnvMut<ExecutionEnv>, req_addr: i64, req_len: i64) {
    let request: fx_common::metrics::CounterIncrementRequest = match decode_memory(&ctx, req_addr, req_len) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to decode memory for metrics counter increment: {err:?}");
            return;
        }
    };

    // TODO: implement this
}

fn api_fetch(ctx: FunctionEnvMut<ExecutionEnv>, req_addr: i64, req_len: i64) -> i64 {
    if !ctx.data().allow_fetch {
        // TODO: handle this properly
        panic!("service {:?} is not allowed to call fetch", ctx.data().service_id.id);
    }

    let request = decode_memory(&ctx, req_addr, req_len)
        .map_err(|err| FxFutureError::SerializationError {
            reason: format!("failed to decode memory: {err:?}"),
        })
        .and_then(|req: HttpRequestInternal| {
            let request = ctx.data().fetch_client
                .request(req.method, req.url.to_string())
                .headers(req.headers);

            if let Some(body) = req.body {
                let stream = ctx.data().engine.streams_pool.read(ctx.data().engine.clone(), &body);
                match stream {
                    Ok(Some(stream)) => Ok(request.body(reqwest::Body::wrap_stream(stream))),
                    Ok(None) => Err(FxFutureError::FetchError {
                        reason: "stream not found".to_owned(),
                    }),
                    Err(err) => Err(FxFutureError::FetchError {
                        reason: format!("failed to read stream: {err:?}"),
                    })
                }
            } else {
                Ok(request)
            }
        });

    let request_future = async move {
        match request {
            Ok(request) => request.send()
                .and_then(|response| async {
                    Ok(rmp_serde::to_vec(&HttpResponse {
                        status: response.status(),
                        headers: response.headers().clone(),
                        body: response.bytes().await.unwrap().to_vec(),
                    }).unwrap())
                })
                .await
                .map_err(|err| FxFutureError::FetchError {
                    reason: format!("request failed: {err:?}"),
                }),
            Err(err) => Err(err),
        }
    }.boxed();

    match ctx.data().engine.futures_pool.push(request_future) {
        Ok(v) => v.0 as i64,
        Err(err) => {
            error!("failed to push future to arena: {err:?}");
            -1
        }
    }
}

fn api_sleep(ctx: FunctionEnvMut<ExecutionEnv>, millis: i64) -> i64 {
    let sleep = tokio::time::sleep(std::time::Duration::from_millis(millis as u64));
    match ctx.data().engine.futures_pool.push(sleep.map(|v| Ok(rmp_serde::to_vec(&v).unwrap())).boxed()) {
        Ok(v) => v.0 as i64,
        Err(err) => {
            error!("failed to push future to arena: {err:?}");
            -1
        }
    }
}

fn api_random(mut ctx: FunctionEnvMut<ExecutionEnv>, len: i64, output_ptr: i64) {
    let mut random_data = vec![0; len as usize];
    rand::rngs::OsRng.try_fill_bytes(&mut random_data).unwrap();

    let (data, mut store) = ctx.data_and_store_mut();
    let len = random_data.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &random_data);
    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
}

fn api_time(_ctx: FunctionEnvMut<ExecutionEnv>) -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

fn api_future_poll(mut ctx: FunctionEnvMut<ExecutionEnv>, index: i64, output_ptr: i64) -> i64 {
    let result = ctx.data().engine.futures_pool.poll(&crate::futures::HostPoolIndex(index as u64), &mut task::Context::from_waker(ctx.data().futures_waker.as_ref().unwrap()));

    match result {
        Poll::Pending => 0,
        Poll::Ready(res) => {
            let (data, mut store) = ctx.data_and_store_mut();
            let res = rmp_serde::to_vec(&res).unwrap();
            let len = res.len() as i64;
            let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
            write_memory(&ctx, ptr, &res);
            write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
            1
        },
    }
}

fn api_future_drop(ctx: FunctionEnvMut<ExecutionEnv>, index: i64) {
    ctx.data().engine.futures_pool.remove(&crate::futures::HostPoolIndex(index as u64));
}

fn api_stream_export(mut ctx: FunctionEnvMut<ExecutionEnv>, output_ptr: i64) {
    let res = ctx.data().engine.streams_pool.push_function_stream(ctx.data().service_id.clone())
        .map_err(|err| fx_common::FxStreamError::PushFailed {
            reason: err.to_string(),
        })
        .map(|v| v.0 as i64);
    let res = rmp_serde::to_vec(&res).unwrap();

    let (data, mut store) = ctx.data_and_store_mut();
    let len = res.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &res);
    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
}

fn api_stream_poll_next(mut ctx: FunctionEnvMut<ExecutionEnv>, index: i64, output_ptr: i64) -> i64 {
    let result = ctx.data().engine.streams_pool.poll_next(
        ctx.data().engine.clone(),
        &crate::streams::HostPoolIndex(index as u64),
        &mut task::Context::from_waker(ctx.data().futures_waker.as_ref().unwrap())
    );

    match result {
        Poll::Pending => 0,
        Poll::Ready(Some(res)) => {
            let res = res.map_err(|err| fx_common::FxStreamError::PollFailed {
                reason: err.to_string(),
            });
            let res = rmp_serde::to_vec(&res).unwrap();

            let (data, mut store) = ctx.data_and_store_mut();
            let len = res.len() as i64;
            let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
            write_memory(&ctx, ptr, &res);
            write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
            1
        },
        Poll::Ready(None) => 2,
    }
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
