use {
    std::{net::SocketAddr, sync::{Arc, Mutex}, convert::Infallible, pin::Pin, fs, ops::DerefMut},
    tokio::{net::TcpListener, sync::oneshot::{self, Sender}},
    hyper_util::rt::tokio::{TokioIo, TokioTimer},
    hyper::{server::conn::http1, Response, body::Bytes},
    http_body_util::Full,
    rayon::{ThreadPool, ThreadPoolBuilder},
    thread_local::ThreadLocal,
    wasmer::{
        wasmparser::Operator,
        Cranelift,
        CompilerConfig,
        Store,
        EngineBuilder,
        Module,
        FunctionEnv,
        FunctionEnvMut,
        Memory,
        Instance,
        Function,
        Value,
        imports,
    },
    wasmer_middlewares::{Metering, metering::{get_remaining_points, set_remaining_points, MeteringPoints}},
    fx_core::{HttpResponse, HttpRequest, LogMessage},
    crate::storage::{KVStorage, SqliteStorage},
};

mod storage;

#[tokio::main]
async fn main() {
    println!("starting fx...");

    let fx_cloud = FxCloud::new();

    let addr: SocketAddr = ([0, 0, 0, 0], 8080).into();
    let listener = TcpListener::bind(addr).await.unwrap();
    println!("running on {addr:?}");
    loop {
        let (tcp, _) = listener.accept().await.unwrap();
        let io = TokioIo::new(tcp);

        let fx_cloud = fx_cloud.clone();
        tokio::task::spawn(async move {
           http1::Builder::new()
               .timer(TokioTimer::new())
               .serve_connection(io, fx_cloud)
               .await
               .unwrap();
        });
    }
}

#[derive(Clone)]
struct FxCloud {
    engine: Arc<Engine>,
}

impl FxCloud {
    pub fn new() -> Self {
        Self {
            engine: Arc::new(Engine::new()),
        }
    }
}

struct Engine {
    thread_pool: ThreadPool,
    execution_context: ThreadLocal<Mutex<ExecutionContext>>,
    storage: SqliteStorage,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            thread_pool: ThreadPoolBuilder::new().build().unwrap(),
            execution_context: ThreadLocal::new(),
            storage: SqliteStorage::new(":memory:"),
        }
    }

    pub fn handle(&self, tx: Sender<Result<Response<Full<Bytes>>, Infallible>>, req: hyper::Request<hyper::body::Incoming>) {
        let ctx = self.execution_context.get_or(|| Mutex::new(self.create_execution_context()));
        let mut ctx = ctx.lock().unwrap();
        let ctx = ctx.deref_mut();

        let points_before = u64::MAX;
        set_remaining_points(&mut ctx.store, &ctx.instance, points_before);

        let memory = ctx.instance.exports.get_memory("memory").unwrap();
        ctx.function_env.as_mut(&mut ctx.store).instance = Some(ctx.instance.clone());
        ctx.function_env.as_mut(&mut ctx.store).memory = Some(memory.clone());

        let client_malloc = ctx.instance.exports.get_function("_fx_malloc").unwrap();

        let request = HttpRequest {
            url: req.uri().to_string(),
        };
        let request = bincode::encode_to_vec(&request, bincode::config::standard()).unwrap();
        let target_addr = client_malloc.call(&mut ctx.store, &[Value::I64(request.len() as i64)]).unwrap()[0].unwrap_i64() as u64;
        memory.view(&mut ctx.store).write(target_addr, &request).unwrap();

        let function = ctx.instance.exports.get_function("_fx_handle").unwrap();
        function.call(&mut ctx.store, &[Value::I64(target_addr as i64), Value::I64(request.len() as i64)]).unwrap();

        let points_used = points_before - match get_remaining_points(&mut ctx.store, &ctx.instance) {
            MeteringPoints::Remaining(v) => v,
            MeteringPoints::Exhausted => panic!("didn't expect that"),
        };
        println!("points used: {:?}", points_used);

        let response_body = ctx.function_env.as_ref(&mut ctx.store).http_response.as_ref().unwrap().body.as_bytes().to_vec();
        tx.send(Ok(Response::new(Full::new(Bytes::from(response_body))))).unwrap();
    }

    fn create_execution_context(&self) -> ExecutionContext {
        ExecutionContext::new(self.storage.clone())
    }
}

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for FxCloud {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        let (tx, rx) = oneshot::channel();
        let engine = self.engine.clone();
        engine.clone().thread_pool.spawn(move || engine.handle(tx, req));

        Box::pin(async move { rx.await.unwrap() })
    }
}

struct ExecutionContext {
    instance: Instance,
    store: Store,
    function_env: FunctionEnv<ExecutionEnv>,
}

impl ExecutionContext {
    pub fn new(storage: SqliteStorage) -> Self {
        let mut compiler_config = Cranelift::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, ops_cost_function)));

        let mut store = Store::new(EngineBuilder::new(compiler_config));

        let module_code = fs::read("./target/wasm32-unknown-unknown/release/fx_app_hello_world.wasm").unwrap();
        let module = Module::new(&store, &module_code).unwrap();

        let function_env = FunctionEnv::new(&mut store, ExecutionEnv::new(storage));
        let import_object = imports! {
            "fx" => {
                "send_http_response" => Function::new_typed_with_env(&mut store, &function_env, api_send_http_response),
                "kv_get" => Function::new_typed_with_env(&mut store, &function_env, api_kv_get),
                "kv_set" => Function::new_typed_with_env(&mut store, &function_env, api_kv_set),
                "log" => Function::new_typed_with_env(&mut store, &function_env, api_log),
            }
        };
        let instance = Instance::new(&mut store, &module, &import_object).unwrap();

        Self {
            instance,
            store,
            function_env,
        }
    }
}

fn ops_cost_function(_: &Operator) -> u64 { 1 }

struct ExecutionEnv {
    instance: Option<Instance>,
    memory: Option<Memory>,
    http_response: Option<HttpResponse>,
    storage: SqliteStorage,
}

impl ExecutionEnv {
    pub fn new(storage: SqliteStorage) -> Self {
        Self {
            instance: None,
            memory: None,
            http_response: None,
            storage,
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

fn decode_memory<T: bincode::de::Decode<()>>(ctx: &FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) -> T {
    bincode::decode_from_slice(&read_memory_owned(&ctx, addr, len), bincode::config::standard()).unwrap().0
}

fn api_send_http_response(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    ctx.data_mut().http_response = Some(decode_memory(&ctx, addr, len));
}

fn api_kv_get(mut ctx: FunctionEnvMut<ExecutionEnv>, k_addr: i64, k_len: i64, output_ptr: i64) -> i64 {
    let key = read_memory_owned(&ctx, k_addr, k_len);
    let value = ctx.data().storage.get(&key);
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
    ctx.data().storage.set(&key, &value);
}

fn api_log(ctx: FunctionEnvMut<ExecutionEnv>, msg_addr: i64, msg_len: i64) {
    let msg: LogMessage = decode_memory(&ctx, msg_addr, msg_len);
    println!("service: {:?}", msg);
}

#[repr(C)]
pub(crate) struct PtrWithLen {
    pub ptr: i64,
    pub len: i64,
}
