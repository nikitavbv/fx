use {
    std::{net::SocketAddr, sync::Arc, convert::Infallible, pin::Pin},
    tokio::{net::TcpListener, sync::oneshot::{self, Sender}},
    hyper_util::rt::tokio::{TokioIo, TokioTimer},
    hyper::{server::conn::http1, Request, Response, body::Bytes},
    http_body_util::Full,
    rayon::{ThreadPool, ThreadPoolBuilder},
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
}

impl Engine {
    pub fn new() -> Self {
        Self {
            thread_pool: ThreadPoolBuilder::new().build().unwrap(),
        }
    }

    pub fn handle(&self, tx: Sender<Result<Response<Full<Bytes>>, Infallible>>, req: hyper::Request<hyper::body::Incoming>) {
        tx.send(Ok(Response::new(Full::new(Bytes::from(format!("hello from thread: {:?}\n", std::thread::current().id())))))).unwrap();
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
