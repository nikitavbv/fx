use {
    std::{convert::Infallible, net::SocketAddr, pin::Pin, sync::{Arc, RwLock}, time::Duration, collections::HashMap},
    tracing::{error, warn},
    tokio::{net::TcpListener, join, time::sleep},
    hyper::{Request, body::{Incoming, Bytes}, Response, server::conn::http1, http::StatusCode},
    hyper_util::rt::{TokioIo, TokioTimer},
    http_body_util::Full,
    thiserror::Error,
    wasmtime::{AsContext, AsContextMut},
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
    crate::runtime::runtime::{Engine, FunctionId},
};

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,

    memory_active: IntGauge,
    memory_resident: IntGauge,

    pub memory_usage_execution_context_create: IntCounter,

    pub http_requests_total: IntCounter,
    pub http_requests_in_flight: IntGauge,
    pub http_functions_in_flight: IntGauge,
    pub http_futures_in_flight: IntGauge,
    pub arena_streams_size: IntGauge,
    pub arena_futures_size: IntGauge,

    pub function_memory_size: IntGaugeVec,
    pub function_memory_pages: IntGaugeVec,
    pub function_execution_context_init_memory_usage: IntGaugeVec,
    pub function_poll_time: IntCounterVec,

    pub function_fx_api_calls: IntCounterVec,

    pub function_metrics: FunctionMetrics,
}

#[derive(Error, Debug)]
pub enum MetricsError {
    #[error("failed to collect: {reason}")]
    FailedToCollect {
        reason: String,
    },
    #[error("failed to register metric")]
    FailedToRegister {
        reason: String,
    },
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let memory_active = register_int_gauge_with_registry!("memory_active", "total number of bytes in active pages allocated by the application", registry).unwrap();
        let memory_resident = register_int_gauge_with_registry!("memory_resident", "total number of bytes in physically resident data pages mapped by the allocator", registry).unwrap();

        let memory_usage_execution_context_create = register_int_counter_with_registry!("memory_usage_execution_context_create", "memory usage increase when creating execution contexts", registry).unwrap();

        let http_requests_total = register_int_counter_with_registry!("http_requests_total", "total http requests processed", registry).unwrap();
        let http_requests_in_flight = register_int_gauge_with_registry!("http_requests_in_flight", "http requests being processed", registry).unwrap();
        let http_functions_in_flight = register_int_gauge_with_registry!("http_functions_in_flight", "http functions being processed", registry).unwrap();
        let http_futures_in_flight = register_int_gauge_with_registry!("http_futures_in_flight", "http futures being processed", registry).unwrap();
        let arena_streams_size = register_int_gauge_with_registry!("arena_streams_size", "size of streams arena", registry).unwrap();
        let arena_futures_size = register_int_gauge_with_registry!("arena_futures_size", "size of futures arena", registry).unwrap();

        let function_memory_size = register_int_gauge_vec_with_registry!("function_memory_size", "size of memory used by function", &["function"], registry).unwrap();
        let function_memory_pages = register_int_gauge_vec_with_registry!("function_memory_pages", "number of memory pages used by function", &["function"], registry).unwrap();
        let function_execution_context_init_memory_usage = register_int_gauge_vec_with_registry!("function_execution_context_init_memory_usage", "memory used to init execution context of a function", &["function"], registry).unwrap();
        let function_poll_time = register_int_counter_vec_with_registry!("function_poll_time", "wall clock time spent polling function future", &["function"], registry).unwrap();

        let function_fx_api_calls = register_int_counter_vec_with_registry!("function_fx_api_calls", "function calls of fx api", &["function", "api"], registry).unwrap();

        Self {
            memory_active,
            memory_resident,

            memory_usage_execution_context_create,

            http_requests_total,
            http_requests_in_flight,
            http_functions_in_flight,
            http_futures_in_flight,
            arena_streams_size,
            arena_futures_size,

            function_memory_size,
            function_memory_pages,
            function_execution_context_init_memory_usage,
            function_poll_time,

            registry: registry.clone(),

            function_fx_api_calls,

            function_metrics: FunctionMetrics::new(registry),
        }
    }

    pub fn encode(&self) -> Result<String, MetricsError> {
        let metrics = self.registry.gather();
        let encoder = TextEncoder::new();
        encoder.encode_to_string(&metrics)
            .map_err(|err| MetricsError::FailedToCollect { reason: format!("{err:?}") })
    }
}

#[derive(Clone)]
pub struct FunctionMetrics {
    registry: Registry,

    counters: Arc<RwLock<HashMap<String, IntCounterVec>>>,
}

impl FunctionMetrics {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,

            counters: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn counter_increment(&self, function_id: &FunctionId, counter_name: &str, tags: Vec<(String, String)>, delta: u64) -> Result<(), MetricsError> {
        let counter_name = format!("{}_{counter_name}", function_id.as_string().replace("-", "_"));
        let key = format!("{counter_name}_{}", tags.iter().map(|(k, _)| k.clone()).collect::<String>());
        let mut counters = self.counters.write()
            .map_err(|err| MetricsError::FailedToRegister { reason: format!("failed to acquire counters lock: {err:?}") })?;
        let tag_values = tags.iter().map(|(_k, v)| v.as_str()).collect::<Vec<&str>>();

        match counters.get(&key) {
            Some(counter) => counter.with_label_values(&tag_values).inc_by(delta),
            None => {
                let counter_opts = prometheus::Opts::new(counter_name.clone(), format!("metric exported by {}", function_id.as_string()));
                let tag_names = tags.iter().map(|(k, _v)| k.as_str()).collect::<Vec<&str>>();
                let counter = IntCounterVec::new(counter_opts, &tag_names)
                    .map_err(|err| MetricsError::FailedToRegister { reason: format!("failed to create counter: {err:?}") })?;
                self.registry.register(Box::new(counter.clone()))
                    .map_err(|err| MetricsError::FailedToRegister { reason: format!("failed to register counter in registry: {err:?}") })?;
                counter.with_label_values(&tag_values).inc_by(delta);
                counters.insert(key.clone(), counter);
            }
        }
        Ok(())
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
                            if err.is_timeout() || err.is_incomplete_message() {
                                // ignore non-critical errors caused by clients
                            } else {
                                error!("error while handling metrics request: {err:?}");
                            }
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
            Ok(execution_contexts) => for (_execution_context_id, execution_env) in execution_contexts.iter() {
                let function_id = execution_env.function_id.as_string();
                let mut store = match execution_env.store.try_lock() {
                    Ok(v) => v,
                    Err(err) => {
                        warn!("failed to lock execution env to collect metrics: {err:?}");
                        continue;
                    }
                };
                let memory = execution_env.instance.get_memory(store.as_context_mut(), "memory");
                if let Some(memory) = memory.as_ref() {
                    let data_size = memory.data_size(store.as_context());
                    metrics.function_memory_size.with_label_values(&[function_id.clone()]).set(data_size as i64);
                    metrics.function_memory_pages.with_label_values(&[function_id.clone()]).set(memory.size(store.as_context()) as i64);
                }
            },
            Err(err) => error!("failed to read execution contexts when collecting metrics: {err:?}"),
        }

        #[cfg(not(target_arch = "aarch64"))]
        {
            if let Err(err) = tikv_jemalloc_ctl::epoch::advance() {
                error!("failed to advance jemalloc_ctl epoch: {err:?}");
            }

            match tikv_jemalloc_ctl::stats::active::read() {
                Ok(v) => metrics.memory_active.set(v as i64),
                Err(err) => {
                    error!("failed to read tikv_jemalloc_ctl::stats::active: {err:?}");
                }
            }

            match tikv_jemalloc_ctl::stats::resident::read() {
                Ok(v) => metrics.memory_resident.set(v as i64),
                Err(err) => {
                    error!("failed to read jemalloc_ctl::stats::resident: {err:?}");
                }
            }
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
