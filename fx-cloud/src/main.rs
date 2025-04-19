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
    fx_core::{HttpResponse, HttpRequest},
};

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
}

impl Engine {
    pub fn new() -> Self {
        Self {
            thread_pool: ThreadPoolBuilder::new().build().unwrap(),
            execution_context: ThreadLocal::new(),
        }
    }

    pub fn handle(&self, tx: Sender<Result<Response<Full<Bytes>>, Infallible>>, req: hyper::Request<hyper::body::Incoming>) {
        let ctx = self.execution_context.get_or(|| Mutex::new(self.create_execution_context()));
        let mut ctx = ctx.lock().unwrap();
        let ctx = ctx.deref_mut();

        let points_before = u64::MAX;
        set_remaining_points(&mut ctx.store, &ctx.instance, points_before);

        let memory = ctx.instance.exports.get_memory("memory").unwrap();
        ctx.function_env.as_mut(&mut ctx.store).memory = Some(memory.clone());

        let client_malloc = ctx.instance.exports.get_function("malloc").unwrap();

        let request = HttpRequest {
            url: req.uri().to_string(),
        };
        let request = bincode::encode_to_vec(&request, bincode::config::standard()).unwrap();
        let target_addr = client_malloc.call(&mut ctx.store, &[Value::I64(request.len() as i64)]).unwrap()[0].unwrap_i64() as u64;
        memory.view(&mut ctx.store).write(target_addr, &request).unwrap();

        let function = ctx.instance.exports.get_function("handle").unwrap();
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
        ExecutionContext::new()
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
    pub fn new() -> Self {
        let cost_function = |_: &Operator| -> u64 { 1 };
        let mut compiler_config = Cranelift::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, cost_function)));

        let mut store = Store::new(EngineBuilder::new(compiler_config));

        let module_code = fs::read("./target/wasm32-unknown-unknown/release/fx_app_hello_world.wasm").unwrap();
        let module = Module::new(&store, &module_code).unwrap();

        let function_env = FunctionEnv::new(&mut store, ExecutionEnv::new());
        let import_object = imports! {
            "fx" => {
                "send_http_response" => Function::new_typed_with_env(&mut store, &function_env, api_send_http_response),
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

struct ExecutionEnv {
    memory: Option<Memory>,
    http_response: Option<HttpResponse>,
}

impl ExecutionEnv {
    pub fn new() -> Self {
        Self {
            memory: None,
            http_response: None,
        }
    }
}

fn api_send_http_response(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    let memory = ctx.data().memory.as_ref().unwrap();
    let view = memory.view(&ctx);

    let addr = addr as u64;
    let len = len as u64;
    let response = view.copy_range_to_vec(addr..addr+len).unwrap();
    let (response, _): (HttpResponse, _) = bincode::decode_from_slice(&response, bincode::config::standard()).unwrap();

    ctx.data_mut().http_response = Some(response);
}
