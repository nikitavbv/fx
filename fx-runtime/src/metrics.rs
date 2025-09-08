use {
    std::{convert::Infallible, net::SocketAddr, pin::Pin, sync::{Arc, RwLock}, time::Duration, collections::HashMap},
    tracing::{error, warn},
    tokio::{net::TcpListener, join, time::sleep},
    hyper::{Request, body::{Incoming, Bytes}, Response, server::conn::http1, http::StatusCode},
    hyper_util::rt::{TokioIo, TokioTimer},
    http_body_util::Full,
    thiserror::Error,
    prometheus::{
        TextEncoder,
        Registry,
        IntGauge,
        IntCounter,
        IntGaugeVec,
        IntCounterVec,
        register_int_gauge_with_registry,
        register_int_gauge_vec_with_registry,
        register_int_counter_with_registry,
        register_int_counter_vec_with_registry,
    },
    crate::cloud::Engine,
};

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,

    pub(crate) http_requests_total: IntCounter,
    pub(crate) http_requests_in_flight: IntGauge,
    pub(crate) http_functions_in_flight: IntGauge,
    pub(crate) http_futures_in_flight: IntGauge,
    pub(crate) arena_streams_size: IntGauge,
    pub(crate) arena_futures_size: IntGauge,

    pub(crate) function_memory_size: IntGaugeVec,
    pub(crate) function_poll_time: IntCounterVec,

    pub(crate) function_metrics: FunctionMetrics,
}

#[derive(Error, Debug)]
pub enum MetricsError {
    #[error("failed to collect: {reason}")]
    FailedToCollect {
        reason: String
    },
}

#[derive(Clone)]
struct FunctionMetrics {
    counters: Arc<RwLock<HashMap<String, IntCounter>>>,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let http_requests_total = register_int_counter_with_registry!("http_requests_total", "total htpt requests processed", registry).unwrap();
        let http_requests_in_flight = register_int_gauge_with_registry!("http_requests_in_flight", "http requests being processed", registry).unwrap();
        let http_functions_in_flight = register_int_gauge_with_registry!("http_functions_in_flight", "http functions being processed", registry).unwrap();
        let http_futures_in_flight = register_int_gauge_with_registry!("http_futures_in_flight", "http futures being processed", registry).unwrap();
        let arena_streams_size = register_int_gauge_with_registry!("arena_streams_size", "size of streams arena", registry).unwrap();
        let arena_futures_size = register_int_gauge_with_registry!("arena_futures_size", "size of futures arena", registry).unwrap();

        let function_memory_size = register_int_gauge_vec_with_registry!("function_memory_size", "size of memory used by function", &["function"], registry).unwrap();
        let function_poll_time = register_int_counter_vec_with_registry!("function_poll_time", "wall clock time spent polling function future", &["function"], registry).unwrap();

        Self {
            http_requests_total,
            http_requests_in_flight,
            http_functions_in_flight,
            http_futures_in_flight,
            arena_streams_size,
            arena_futures_size,

            function_memory_size,
            function_poll_time,

            registry,

            function_metrics: FunctionMetrics {
                counters: Arc::new(RwLock::new(HashMap::new())),
            }
        }
    }

    pub fn encode(&self) -> Result<String, MetricsError> {
        let metrics = self.registry.gather();
        let encoder = TextEncoder::new();
        encoder.encode_to_string(&metrics)
            .map_err(|err| MetricsError::FailedToCollect { reason: format!("{err:?}") })
    }
}

#[allow(dead_code)]
pub async fn run_metrics_server(engine: Arc<Engine>, port: u16) {
    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = match TcpListener::bind(addr).await {
        Ok(v) => v,
        Err(err) => {
            error!("failed to create TcpListener for metrics server: {err:?}");
            return;
        }
    };

    println!("running metrics server on {addr:?}");

    let metrics_server = MetricsServer::new(engine.metrics.clone());

    join!(
        collect_metrics(engine.clone()),
        async {
            loop {
                let (tcp, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(err) => {
                        error!("failed to accept connection in metrics server: {err:?}");
                        continue;
                    }
                };
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
    );
}

async fn collect_metrics(engine: Arc<Engine>) {
    let metrics = engine.metrics.clone();
    loop {
        match engine.streams_pool.len() {
            Ok(v) => metrics.arena_streams_size.set(v as i64),
            Err(err) => {
                error!("failed to get streams arena size: {err:?}");
            }
        }
        match engine.futures_pool.len() {
            Ok(v) => metrics.arena_futures_size.set(v as i64),
            Err(err) => {
                error!("failed to get futures arena size: {err:?}");
            }
        }

        match engine.execution_contexts.read() {
            Ok(execution_contexts) => for (function_id, execution_env) in execution_contexts.iter() {
                let function_id = function_id.as_string();
                let store = match execution_env.store.try_lock() {
                    Ok(v) => v,
                    Err(err) => {
                        warn!("failed to lock execution env to collect metrics: {err:?}");
                        continue;
                    }
                };
                let memory = execution_env.function_env.as_ref(&store).memory.as_ref();
                if let Some(memory) = memory.as_ref() {
                    let data_size = memory.view(&store).data_size();
                    metrics.function_memory_size.with_label_values(&[function_id]).set(data_size as i64);
                }
            },
            Err(err) => error!("failed to read execution contexts when collecting metrics: {err:?}"),
        }

        sleep(Duration::from_secs(10)).await;
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
    type Error = MetricsError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, _req: Request<Incoming>) -> Self::Future {
        let metrics = match self.metrics.encode() {
            Ok(v) => v,
            Err(err) => {
                error!("failed to encode metrics: {err:?}");
                return Box::pin(async move {
                    let mut response = Response::new(Full::new(Bytes::from("interal server error.\n")));
                    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    Ok(response)
                });
            }
        };
        Box::pin(async move { Ok(Response::new(Full::new(Bytes::from(metrics)))) })
    }
}
