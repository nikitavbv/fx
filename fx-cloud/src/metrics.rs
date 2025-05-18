use {
    std::{convert::Infallible, net::SocketAddr, pin::Pin},
    tracing::error,
    tokio::net::TcpListener,
    hyper::{Request, body::{Incoming, Bytes}, Response, server::conn::http1},
    hyper_util::rt::{TokioIo, TokioTimer},
    http_body_util::Full,
    prometheus::{
        TextEncoder,
        Registry,
        IntGauge,
        IntCounter,
        register_int_gauge_with_registry,
        register_int_counter_with_registry,
    },
    crate::FxCloud,
};

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,

    pub(crate) http_requests_total: IntCounter,
    pub(crate) http_requests_in_flight: IntGauge,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let http_requests_total = register_int_counter_with_registry!("http_requests_total", "total htpt requests processed", registry).unwrap();
        let http_requests_in_flight = register_int_gauge_with_registry!("http_requests_in_flight", "http requests being processed", registry).unwrap();

        Self {
            http_requests_total,
            http_requests_in_flight,
            registry,
        }
    }

    pub fn encode(&self) -> String {
        let metrics = self.registry.gather();
        let encoder = TextEncoder::new();
        encoder.encode_to_string(&metrics).unwrap()
    }
}

impl FxCloud {
    pub async fn run_metrics_server(&self, port: u16) {
        let addr: SocketAddr = ([0, 0, 0, 0], port).into();
        let listener = TcpListener::bind(addr).await.unwrap();

        println!("running metrics server on {addr:?}");

        let metrics_server = MetricsServer::new(self.engine.metrics.clone());

        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(tcp);
            let metrics_server = metrics_server.clone();
            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .timer(TokioTimer::new())
                    .serve_connection(io, metrics_server)
                    .await {
                        error!("error while handling metrics request: {err:?}");
                    }
            });
        }
    }
}

#[derive(Clone)]
struct MetricsServer {
    metrics: Metrics,
}

impl MetricsServer {
    pub fn new(metrics: Metrics) -> Self {
        Self {
            metrics,
        }
    }
}

impl hyper::service::Service<Request<Incoming>> for MetricsServer {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, _req: Request<Incoming>) -> Self::Future {
        let metrics = self.metrics.encode();
        Box::pin(async move { Ok(Response::new(Full::new(Bytes::from(metrics)))) })
    }
}
