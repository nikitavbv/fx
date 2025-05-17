use {
    std::{net::SocketAddr, sync::{Arc, Mutex, RwLock, atomic::{AtomicBool, Ordering}}, collections::HashMap, ops::DerefMut, task::{self, Poll}, cell::RefCell},
    tracing::error,
    tokio::net::TcpListener,
    hyper_util::rt::tokio::{TokioIo, TokioTimer},
    hyper::server::conn::http1,
    thread_local::ThreadLocal,
    wasmer::{
        wasmparser::Operator,
        Cranelift,
        CompilerConfig,
        Store,
        EngineBuilder,
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
    futures::FutureExt,
    fx_core::{LogMessage, DatabaseSqlQuery, DatabaseSqlBatchQuery, SqlResult, SqlResultRow, SqlValue, FetchRequest, HttpResponse},
    fx_cloud_common::FunctionInvokeEvent,
    crate::{
        storage::{KVStorage, NamespacedStorage, EmptyStorage, BoxedStorage},
        error::FxCloudError,
        http::HttpHandler,
        compatibility,
        sql::{self, SqlDatabase},
        queue::{Queue, AsyncRpcMessage, QUEUE_RPC},
        cron::CronRunner,
        compiler::{Compiler, BoxedCompiler, SimpleCompiler, MemoizedCompiler},
        futures::FuturesPool,
        streams::StreamsPool,
    },
};

pub const QUEUE_SYSTEM_INVOCATIONS: &str = "system/invocations";

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

    pub fn with_service(self, service: Service) -> Self {
        {
            let mut services = self.engine.services.write().unwrap();
            services.insert(service.id.clone(), service);
        }
        self
    }

    pub fn with_code_storage(self, new_storage: BoxedStorage) -> Self {
        {
            let mut storage = self.engine.module_code_storage.write().unwrap();
            *storage = new_storage;
        }
        self
    }

    pub fn with_memoized_compiler(self, compiled_code_storage: BoxedStorage) -> Self {
        {
            let mut compiler = self.engine.compiler.write().unwrap();
            let prev_compiler = std::mem::replace(&mut *compiler, BoxedCompiler::new(SimpleCompiler::new()));
            *compiler = BoxedCompiler::new(MemoizedCompiler::new(compiled_code_storage, prev_compiler));
        }
        self
    }

    pub async fn with_queue(self) -> Self {
        let engine = self.engine.clone();
        *self.engine.queue.write().await = Some(Queue::new(engine));
        self
    }

    pub async fn with_queue_subscription(self, queue_id: impl Into<String>, function_id: ServiceId, rpc_function_name: impl Into<String>) -> Self {
        self.engine.queue.read().await.as_ref().unwrap().subscribe(queue_id.into(), function_id, rpc_function_name.into()).await;
        self
    }

    pub fn with_cron(self, sql: SqlDatabase) -> Result<Self, FxCloudError> {
        *self.engine.cron.write().unwrap() = Some(CronRunner::new(self.engine.clone(), sql)?);
        Ok(self)
    }

    pub fn with_cron_task(self, cron_expression: impl Into<String>, function_id: ServiceId, rpc_function_name: impl Into<String>) -> Result<Self, FxCloudError> {
        self.engine.cron.read().unwrap().as_ref().unwrap().schedule(cron_expression, function_id, rpc_function_name.into())?;
        Ok(self)
    }

    #[allow(dead_code)]
    pub async fn invoke_service<T: serde::ser::Serialize, S: serde::de::DeserializeOwned>(&self, service: &ServiceId, function_name: &str, argument: T) -> Result<S, FxCloudError> {
        self.engine.invoke_service(self.engine.clone(), service, function_name, argument).await
    }

    pub fn invoke_service_raw(&self, service: &ServiceId, function_name: &str, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FxCloudError> {
        self.engine.invoke_service_raw(self.engine.clone(), service.clone(), function_name.to_owned(), argument)
    }

    pub async fn invoke_service_async<T: serde::ser::Serialize>(&self, function_id: ServiceId, rpc_function_name: String, argument: T) {
        self.engine.invoke_service_async(function_id, rpc_function_name, argument).await;
    }

    pub async fn run_http(&self, port: u16, service_id: ServiceId) {
        let addr: SocketAddr = ([0, 0, 0, 0], port).into();
        let listener = TcpListener::bind(addr).await.unwrap();

        let http_handler = HttpHandler::new(self.clone(), service_id);

        println!("running on {addr:?}");
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(tcp);

            let http_handler = http_handler.clone();
            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                   .timer(TokioTimer::new())
                   .serve_connection(io, http_handler)
                   .await {
                        if err.is_timeout() {
                            // ignore timeouts, because those can be caused by client
                        } else {
                            error!("error while handling http request: {err:?}");
                        }
                   }
            });
        }
    }

    pub async fn run_queue(&self) {
        self.engine.queue.read().await.as_ref().unwrap().clone().run();
    }

    pub fn run_cron(&self) {
        self.engine.cron.read().unwrap().as_ref().unwrap().clone().run();
    }
}

#[derive(Hash, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub struct ServiceId {
    id: String,
}

impl ServiceId {
    pub fn new(id: String) -> Self {
        Self {
            id,
        }
    }
}

impl Into<String> for ServiceId {
    fn into(self) -> String {
        self.id
    }
}

pub struct Service {
    id: ServiceId,
    env_vars: HashMap<Vec<u8>, Vec<u8>>,
    is_system: bool,
    is_global: bool,
    storage: BoxedStorage,
    sql: HashMap<String, SqlDatabase>,
    allow_fetch: bool,
    allow_log: bool,
}

impl Service {
    pub fn new(id: ServiceId) -> Self {
        Self {
            id,
            env_vars: HashMap::new(),
            is_system: false,
            is_global: false,
            storage: BoxedStorage::new(EmptyStorage),
            sql: HashMap::new(),
            allow_fetch: false,
            allow_log: true,
        }
    }

    pub fn with_env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.insert(key.into().into_bytes(), value.into().into_bytes());
        self
    }

    // system functions do not use common even infrastructure to avoid recursive event generation
    pub fn system(mut self) -> Self {
        self.is_system = true;
        self
    }

    // global service is a service that only has single instance. stateful actor.
    pub fn global(mut self) -> Self {
        self.is_global = true;
        self
    }

    pub fn with_storage(mut self, storage: BoxedStorage) -> Self {
        self.storage = storage;
        self
    }

    pub fn with_sql_database(mut self, name: String, database: SqlDatabase) -> Self {
        self.sql.insert(name, database);
        self
    }

    pub fn get_storage(&self) -> BoxedStorage {
        self.storage.clone()
    }

    pub fn allow_fetch(mut self) -> Self {
        self.allow_fetch = true;
        self
    }

    #[allow(dead_code)]
    pub fn disallow_log(mut self) -> Self {
        self.allow_log = false;
        self
    }
}

pub(crate) struct Engine {
    compiler: RwLock<BoxedCompiler>,

    execution_contexts: ThreadLocal<Mutex<HashMap<ServiceId, Arc<ExecutionContext>>>>,
    global_execution_contexts: RwLock<HashMap<ServiceId, Arc<ExecutionContext>>>,

    services: RwLock<HashMap<ServiceId, Service>>,

    queue: tokio::sync::RwLock<Option<Queue>>,

    cron: RwLock<Option<CronRunner>>,

    // internal storage where .wasm is loaded from:
    module_code_storage: RwLock<BoxedStorage>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            compiler: RwLock::new(BoxedCompiler::new(SimpleCompiler::new())),

            execution_contexts: ThreadLocal::new(),
            global_execution_contexts: RwLock::new(HashMap::new()),

            services: RwLock::new(HashMap::new()),

            queue: tokio::sync::RwLock::new(None),

            cron: RwLock::new(None),

            module_code_storage: RwLock::new(BoxedStorage::new(NamespacedStorage::new(b"services/", EmptyStorage))),
        }
    }

    pub async fn invoke_service<T: serde::ser::Serialize, S: serde::de::DeserializeOwned>(&self, engine: Arc<Engine>, service: &ServiceId, function_name: &str, argument: T) -> Result<S, FxCloudError> {
        let argument = rmp_serde::to_vec(&argument).unwrap();
        let response = self.invoke_service_raw(engine, service.clone(), function_name.to_owned(), argument)?.await?;
        Ok(rmp_serde::from_slice(&response).unwrap())
    }

    pub fn invoke_service_raw(&self, engine: Arc<Engine>, service: ServiceId, function_name: String, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FxCloudError> {
        if self.is_global_service(&service)? {
            self.invoke_global_service(engine, &service, &function_name, argument)
        } else {
            self.invoke_thread_local_service(engine, &service, &function_name, argument)
        }
    }

    pub async fn invoke_service_async<T: serde::ser::Serialize>(&self, function_id: ServiceId, rpc_function_name: String, argument: T) {
        self.push_to_queue(QUEUE_RPC, AsyncRpcMessage {
            function_id,
            rpc_function_name,
            argument: rmp_serde::to_vec(&argument).unwrap(),
        }).await;
    }

    fn invoke_global_service(&self, engine: Arc<Engine>, service_id: &ServiceId, function_name: &str, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FxCloudError> {
        if !self.global_execution_contexts.read().unwrap().contains_key(service_id) {
            // need to create execution context first
            let mut global_execution_contexts = self.global_execution_contexts.write().unwrap();
            let ctx = global_execution_contexts.get(&service_id);
            if ctx.map(|v| v.needs_recreate.load(Ordering::SeqCst)).unwrap_or(true) {
                let services = self.services.read().unwrap();
                let service = services.get(&service_id).unwrap();
                global_execution_contexts.insert(service_id.clone(), Arc::new(self.create_execution_context(engine.clone(), &service)?));
            }
        }

        let ctxs = self.global_execution_contexts.read().unwrap();
        let ctx = ctxs.get(&service_id).unwrap().clone();
        drop(ctxs);

        Ok(self.run_service(engine, ctx, function_name, argument))
    }

    fn invoke_thread_local_service(&self, engine: Arc<Engine>, service_id: &ServiceId, function_name: &str, argument: Vec<u8>) -> Result<FunctionRuntimeFuture, FxCloudError> {
        let ctxs = self.execution_contexts.get_or(|| Mutex::new(HashMap::new()));

        // this lock is cheap, because map is per-thread, so we expect this lock to always be unlocked
        let ctx = {
            let mut ctxs = ctxs.lock().unwrap();
            let ctx = ctxs.get(&service_id);
            if ctx.map(|v| v.needs_recreate.load(Ordering::SeqCst)).unwrap_or(true) {
                let services = self.services.read().unwrap();
                let service = services.get(&service_id).unwrap();
                ctxs.insert(service_id.clone(), Arc::new(self.create_execution_context(engine.clone(), &service)?));
            }
            let ctx = ctxs.get(&service_id);
            let ctx = ctx.unwrap().clone();
            ctx
        };
        Ok(self.run_service(engine, ctx, function_name, argument))
    }

    fn run_service(&self, engine: Arc<Engine>, ctx: Arc<ExecutionContext>, function_name: &str, argument: Vec<u8>) -> FunctionRuntimeFuture {
        FunctionRuntimeFuture {
            engine,
            ctx,
            function_name: function_name.to_owned(),
            argument: argument.to_owned(),
            rpc_future_index: Arc::new(Mutex::new(None)),
        }
    }

    fn create_execution_context(&self, engine: Arc<Engine>, service: &Service) -> Result<ExecutionContext, FxCloudError> {
        let module_code = self.module_code_storage.read().unwrap().get(service.id.id.as_bytes())?;
        let module_code = match module_code {
            Some(v) => v,
            None => return Err(FxCloudError::ModuleCodeNotFound),
        };

        ExecutionContext::new(
            engine,
            service.id.clone(),
            service.is_system,
            service.get_storage(),
            service.sql.clone(),
            module_code,
            service.env_vars.clone(),
            service.allow_fetch,
            service.allow_log,
        )
    }

    fn is_global_service(&self, service_id: &ServiceId) -> Result<bool, FxCloudError> {
        Ok(self.services.read().unwrap().get(service_id).as_ref()
            .ok_or(FxCloudError::ServiceNotFound)?
            .is_global)
    }

    async fn push_to_queue<T: serde::ser::Serialize>(&self, queue_id: impl Into<String>, message: T) {
        self.push_to_queue_raw(queue_id, rmp_serde::to_vec(&message).unwrap()).await;
    }

    async fn push_to_queue_raw(&self, queue_id: impl Into<String>, message: Vec<u8>) {
        let queue = self.queue.read().await;
        let queue = match queue.as_ref() {
            Some(v) => v,
            None => return,
        };
        queue.push(queue_id.into(), message).await;
    }
}

pub struct FunctionRuntimeFuture {
    engine: Arc<Engine>,
    ctx: Arc<ExecutionContext>,
    function_name: String,
    argument: Vec<u8>,
    rpc_future_index: Arc<Mutex<Option<i64>>>,
}

impl FunctionRuntimeFuture {
    fn record_function_invocation(&self) {
        if self.ctx.is_system {
            // invocations are not recorded for system functions to avoid loops
            return;
        }

        let engine = self.engine.clone();
        let message = FunctionInvokeEvent {
            function_id: self.ctx.service_id.id.clone(),
        };
        tokio::runtime::Handle::current().spawn(async move {
            engine.push_to_queue(QUEUE_SYSTEM_INVOCATIONS, message).await;
        });
    }
}

impl Future for FunctionRuntimeFuture {
    type Output = Result<Vec<u8>, FxCloudError>;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let rpc_future_index = self.rpc_future_index.clone();
        let mut rpc_future_index = rpc_future_index.lock().unwrap();

        let argument = self.argument.clone();
        let function_name = self.function_name.clone();
        let ctx = &self.ctx;
        let mut store_lock = ctx.store.lock().unwrap();
        let store = store_lock.deref_mut();

        {
            let function_env = ctx.function_env.as_ref(store);
            if function_env.execution_context.read().unwrap().is_none() {
                let mut f_env_execution_context = function_env.execution_context.write().unwrap();
                *f_env_execution_context = Some(ctx.clone());
            }
        }

        let function_poll = ctx.instance.exports.get_function("_fx_future_poll").unwrap();

        if let Some(rpc_future_index) = rpc_future_index.as_ref() {
            // TODO: measure points
            let poll_is_ready = function_poll.call(store, &[Value::I64(*rpc_future_index)]).unwrap()[0].unwrap_i64();
            let result = if poll_is_ready == 1 {
                let response = ctx.function_env.as_ref(store).rpc_response.as_ref().unwrap().clone();
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

            // TODO: errors like this should be reported to some data stream
            let future_index = function.call(store, &[Value::I64(target_addr as i64), Value::I64(argument.len() as i64)])
                .map_err(|err| FxCloudError::ServiceInternalError { reason: format!("rpc call failed: {err:?}") });
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

            result
        }
    }
}

#[derive(Clone)]
pub(crate) struct ExecutionContext {
    instance: Instance,
    store: Arc<Mutex<Store>>,
    function_env: FunctionEnv<ExecutionEnv>,
    service_id: ServiceId,
    is_system: bool,
    needs_recreate: Arc<AtomicBool>,
}

impl ExecutionContext {
    pub fn new(
        engine: Arc<Engine>,
        service_id: ServiceId,
        is_system: bool,
        storage: BoxedStorage,
        sql: HashMap<String, SqlDatabase>,
        module_code: Vec<u8>,
        env_vars: HashMap<Vec<u8>, Vec<u8>>,
        allow_fetch: bool,
        allow_log: bool
    ) -> Result<Self, FxCloudError> {
        let futures = FuturesPool::new();
        let streams = StreamsPool::new();

        let mut compiler_config = Cranelift::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, ops_cost_function)));

        let mut store = Store::new(EngineBuilder::new(compiler_config));

        let module = engine.compiler.read().unwrap().compile(&store, module_code);
        let function_env = FunctionEnv::new(&mut store, ExecutionEnv::new(futures, streams, engine, service_id.clone(), env_vars, storage, sql, allow_fetch, allow_log));

        let mut import_object = imports! {
            "fx" => {
                "rpc" => Function::new_typed_with_env(&mut store, &function_env, api_rpc),
                "rpc_async" => Function::new_typed_with_env(&mut store, &function_env, api_rpc_async),
                "send_rpc_response" => Function::new_typed_with_env(&mut store, &function_env, api_send_rpc_response),
                "kv_get" => Function::new_typed_with_env(&mut store, &function_env, api_kv_get),
                "kv_set" => Function::new_typed_with_env(&mut store, &function_env, api_kv_set),
                "sql_exec" => Function::new_typed_with_env(&mut store, &function_env, api_sql_exec),
                "sql_batch" => Function::new_typed_with_env(&mut store, &function_env, api_sql_batch),
                "queue_push" => Function::new_typed_with_env(&mut store, &function_env, api_queue_push),
                "log" => Function::new_typed_with_env(&mut store, &function_env, api_log),
                "fetch" => Function::new_typed_with_env(&mut store, &function_env, api_fetch),
                "sleep" => Function::new_typed_with_env(&mut store, &function_env, api_sleep),
                "future_poll" => Function::new_typed_with_env(&mut store, &function_env, api_future_poll),
                "stream_export" => Function::new_typed_with_env(&mut store, &function_env, api_stream_export),
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

        let instance = Instance::new(&mut store, &module, &import_object)
            .map_err(|err| FxCloudError::CompilationError { reason: format!("failed to create wasm instance: {err:?}") })?;

        Ok(Self {
            instance,
            store: Arc::new(Mutex::new(store)),
            function_env,
            service_id,
            is_system,
            needs_recreate: Arc::new(AtomicBool::new(false)),
        })
    }
}

fn ops_cost_function(_: &Operator) -> u64 { 1 }

pub(crate) struct ExecutionEnv {
    execution_context: RwLock<Option<Arc<ExecutionContext>>>,

    futures: FuturesPool,
    futures_waker: Option<std::task::Waker>,
    streams: StreamsPool,

    engine: Arc<Engine>,
    instance: Option<Instance>,
    memory: Option<Memory>,
    rpc_response: Option<Vec<u8>>,

    service_id: ServiceId,
    env_vars: HashMap<Vec<u8>, Vec<u8>>,

    storage: BoxedStorage,
    sql: HashMap<String, SqlDatabase>,

    allow_fetch: bool,
    allow_log: bool,

    fetch_client: reqwest::Client,
}

impl ExecutionEnv {
    pub fn new(
        futures: FuturesPool,
        streams: StreamsPool,
        engine: Arc<Engine>,
        service_id: ServiceId,
        env_vars: HashMap<Vec<u8>, Vec<u8>>,
        storage: BoxedStorage,
        sql: HashMap<String, SqlDatabase>,
        allow_fetch: bool,
        allow_log: bool
    ) -> Self {
        Self {
            execution_context: RwLock::new(None),
            futures,
            futures_waker: None,
            streams,
            engine,
            instance: None,
            memory: None,
            rpc_response: None,
            service_id,
            env_vars,
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

fn decode_memory<T: serde::de::DeserializeOwned>(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) -> T {
    let memory = read_memory_owned(&ctx, addr, len);
    rmp_serde::from_slice(&memory).unwrap()
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
    let service_id = ServiceId::new(String::from_utf8(read_memory_owned(&ctx, service_name_ptr, service_name_len)).unwrap());
    let function_name = String::from_utf8(read_memory_owned(&ctx, function_name_ptr, function_name_len)).unwrap();
    let argument = read_memory_owned(&ctx, arg_ptr, arg_len);

    let engine = ctx.data().engine.clone();
    let response_future = engine.clone().invoke_service_raw(engine.clone(), service_id, function_name, argument).unwrap();
    let response_future = ctx.data().futures.push(response_future.map(|v| v.unwrap()).boxed());

    response_future.0 as i64
}

fn api_rpc_async(
    ctx: FunctionEnvMut<ExecutionEnv>,
    service_name_ptr: i64,
    service_name_len: i64,
    function_name_ptr: i64,
    function_name_len: i64,
    arg_ptr: i64,
    arg_len: i64
) {
    // TODO: permissions check
    let service_id = ServiceId::new(String::from_utf8(read_memory_owned(&ctx, service_name_ptr, service_name_len)).unwrap());
    let function_name = String::from_utf8(read_memory_owned(&ctx, function_name_ptr, function_name_len)).unwrap();
    let argument = read_memory_owned(&ctx, arg_ptr, arg_len);

    let engine = ctx.data().engine.clone();
    tokio::task::spawn(async move { engine.push_to_queue(QUEUE_RPC, AsyncRpcMessage {
        function_id: service_id,
        rpc_function_name: function_name,
        argument,
    }).await });
}

fn api_send_rpc_response(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    ctx.data_mut().rpc_response = Some(read_memory_owned(&ctx, addr, len));
}

fn api_kv_get(mut ctx: FunctionEnvMut<ExecutionEnv>, k_addr: i64, k_len: i64, output_ptr: i64) -> i64 {
    let key = read_memory_owned(&ctx, k_addr, k_len);
    let value = {
        let ctx = ctx.data();
        if let Some(value) = ctx.env_vars.get(&key) {
            Some(value.clone())
        } else {
            ctx.storage.get(&key).unwrap()
        }
    };
    let value = match value {
        Some(v) => v,
        None => return 1,
    };

    let (data, mut store) = ctx.data_and_store_mut();

    let len = value.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &value);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });

    0
}

fn api_kv_set(ctx: FunctionEnvMut<ExecutionEnv>, k_addr: i64, k_len: i64, v_addr: i64, v_len: i64) {
    let key = read_memory_owned(&ctx, k_addr, k_len);
    let value = read_memory_owned(&ctx, v_addr, v_len);
    // TODO: report errors to calling service
    ctx.data().storage.set(&key, &value).unwrap();
}

fn api_sql_exec(mut ctx: FunctionEnvMut<ExecutionEnv>, query_addr: i64, query_len: i64, output_ptr: i64) {
    let request: DatabaseSqlQuery = decode_memory(&ctx, query_addr, query_len);

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

    // TODO: report errors to calling service
    let result = ctx.data().sql.get(&request.database).as_ref().unwrap().exec(query).unwrap();
    let result = SqlResult {
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
    };

    let result = rmp_serde::to_vec(&result).unwrap();

    let (data, mut store) = ctx.data_and_store_mut();

    let len = result.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &result);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
}

fn api_sql_batch(ctx: FunctionEnvMut<ExecutionEnv>, query_addr: i64, query_len: i64) {
    let request: DatabaseSqlBatchQuery = decode_memory(&ctx, query_addr, query_len);

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
    ctx.data().sql.get(&request.database).as_ref().unwrap().batch(queries).unwrap();
}

fn api_queue_push(ctx: FunctionEnvMut<ExecutionEnv>, queue_addr: i64, queue_len: i64, argument_addr: i64, argument_len: i64) {
    let queue = String::from_utf8(read_memory_owned(&ctx, queue_addr, queue_len)).unwrap();
    let argument = read_memory_owned(&ctx, argument_addr, argument_len);
    let engine = ctx.data().engine.clone();
    tokio::task::spawn(async move { engine.push_to_queue_raw(queue, argument).await });
}

fn api_log(ctx: FunctionEnvMut<ExecutionEnv>, msg_addr: i64, msg_len: i64) {
    if !ctx.data().allow_log {
        // TODO: record a metric somewhere
        return;
    }
    let msg: LogMessage = decode_memory(&ctx, msg_addr, msg_len);
    println!("service: {:?}", msg);
}

fn api_fetch(ctx: FunctionEnvMut<ExecutionEnv>, req_addr: i64, req_len: i64) -> i64 {
    if !ctx.data().allow_fetch {
        // TODO: handle this properly
        panic!("service {:?} is not allowed to call fetch", ctx.data().service_id.id);
    }

    let req: FetchRequest = decode_memory(&ctx, req_addr, req_len);

    let mut request = ctx.data().fetch_client.request(
        match req.method {
            fx_core::HttpMethod::GET => reqwest::Method::GET,
            fx_core::HttpMethod::POST => reqwest::Method::POST,
        }, req.endpoint
    );
    for (header_name, header_value) in req.headers {
        request = request.header(header_name, header_value);
    }
    if let Some(body) = req.body {
        request = request.body(body);
    }

    let request_future = request.send()
        .then(|response| async {
            let response = response.unwrap();

            rmp_serde::to_vec(&HttpResponse {
                status: response.status(),
                headers: response.headers().clone(),
                body: response.bytes().await.unwrap().to_vec(),
            }).unwrap()
        })
        .boxed();

    ctx.data().futures.push(request_future).0 as i64
}

fn api_sleep(ctx: FunctionEnvMut<ExecutionEnv>, millis: i64) -> i64 {
    let sleep = tokio::time::sleep(std::time::Duration::from_millis(millis as u64));
    let future_index = ctx.data().futures.push(sleep.map(|v| rmp_serde::to_vec(&v).unwrap()).boxed());
    future_index.0 as i64
}

fn api_future_poll(mut ctx: FunctionEnvMut<ExecutionEnv>, index: i64, output_ptr: i64) -> i64 {
    let result = ctx.data().futures.poll(&crate::futures::HostPoolIndex(index as u64), &mut task::Context::from_waker(ctx.data().futures_waker.as_ref().unwrap()));

    match result {
        Poll::Pending => 0,
        Poll::Ready(res) => {
            let (data, mut store) = ctx.data_and_store_mut();
            let len = res.len() as i64;
            let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
            write_memory(&ctx, ptr, &res);
            write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
            1
        },
    }
}

fn api_stream_export(ctx: FunctionEnvMut<ExecutionEnv>) -> i64 {
    let execution_context = ctx.data().execution_context.read().unwrap().clone().unwrap();
    let index = ctx.data().streams.push_function_stream(execution_context);
    println!("exported stream: {index:?}");
    index.0 as i64
}

fn api_list_functions(mut ctx: FunctionEnvMut<ExecutionEnv>, output_ptr: i64) {
    // TODO: permissions check
    let functions: Vec<_> = ctx.data().engine.services.read().unwrap()
        .iter()
        .map(|(function_id, _function)| fx_cloud_common::Function {
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
