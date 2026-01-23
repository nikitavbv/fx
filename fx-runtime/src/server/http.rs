use {
    std::{convert::Infallible, pin::Pin, time::{Instant, Duration}, sync::{Arc, Mutex}},
    tracing::{error, warn},
    hyper::{Response, body::Bytes, StatusCode},
    http_body_util::{Full, BodyStream},
    futures::{StreamExt, stream::BoxStream, future::BoxFuture, FutureExt},
    tokio::time::timeout,
    arc_swap::ArcSwap,
    fx_common::{HttpResponse, HttpRequest, FxStream},
    crate::common::{FunctionInvokeEvent, events::InvocationTimings},
    crate::runtime::{FxRuntime, FunctionId, error::FxRuntimeError, runtime::{Engine, FunctionInvokeAndExecuteError}},
};

pub struct HttpHandler {
    fx: Arc<FxRuntime>,
    function_id: ArcSwap<FunctionId>,
}

impl HttpHandler {
    #[allow(dead_code)]
    pub fn new(fx: Arc<FxRuntime>, function_id: FunctionId) -> Self {
        Self {
            fx,
            function_id: ArcSwap::from_pointee(function_id),
        }
    }

    pub fn update_target_function(&self, function_id: FunctionId) {
        self.function_id.store(Arc::new(function_id));
    }
}

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for HttpHandler {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        Box::pin(HttpHandlerFuture::new(self.fx.engine.clone(), self.function_id.load().clone(), req))
    }
}

struct HttpHandlerFuture<'a> {
    inner: BoxFuture<'a, Result<Response<Full<Bytes>>, Infallible>>,
}

impl<'a> HttpHandlerFuture<'a> {
    fn new(engine: Arc<Engine>, service_id: Arc<FunctionId>, req: hyper::Request<hyper::body::Incoming>) -> Self {
        let started_at = Instant::now();

        engine.metrics.http_requests_in_flight.inc();
        let metric_guard_http_requests_in_flight = MetricGaugeDecreaseGuard::wrap(engine.metrics.http_requests_in_flight.clone());

        let inner = Box::pin(async move {
            engine.metrics.http_futures_in_flight.inc();
            let metric_guard_http_futures_in_flight = MetricGaugeDecreaseGuard::wrap(engine.metrics.http_futures_in_flight.clone());

            let method = req.method().to_owned();
            let url = req.uri().clone();
            let headers = req.headers().clone();
            let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok()).map(|v| v.to_owned());

            let body: BoxStream<'static, Result<Vec<u8>, FxRuntimeError>> = BodyStream::new(req.into_body())
                .map(|v| v
                    .map_err(|err| FxRuntimeError::StreamingError { reason: format!("failed to convert body into stream: {err:?}") })
                    .and_then(|v| v.into_data().map_err(|err| FxRuntimeError::StreamingError { reason: format!("failed to convert body stream into data: {err:?}") }).map(|v| v.to_vec()))
                )
                .boxed();

            let (fx_response, invocation_event) = match engine.streams_pool.push(body) {
                Ok(body_stream_index) => {
                    let body_stream_cleanup_guard = StreamPoolCleanupGuard::wrap(engine.clone(), body_stream_index.clone());
                    let body = FxStream { index: body_stream_index.0 as i64 };

                    let request = HttpRequest {
                        method,
                        url,
                        headers,
                        body: Some(body),
                    };

                    engine.metrics.http_functions_in_flight.inc();
                    let metric_guard_http_functions_in_flight = MetricGaugeDecreaseGuard::wrap(engine.metrics.http_functions_in_flight.clone());

                    let invoke_service_future = engine.invoke_service::<_, HttpResponse>(engine.clone(), &service_id, "http", request);
                    let invoke_service_with_timeout = timeout(Duration::from_secs(60), invoke_service_future);
                    let (fx_response, invocation_event) = match invoke_service_with_timeout.await {
                        Ok(v) => match v {
                            Ok(v) => (v.0, Some(v.1)),
                            Err(err) => (match err {
                                FunctionInvokeAndExecuteError::RuntimeError
                                | FunctionInvokeAndExecuteError::FunctionRuntimeError => {
                                    response_internal_error()
                                },
                                FunctionInvokeAndExecuteError::UserApplicationError { description: _ }
                                | FunctionInvokeAndExecuteError::FunctionPanicked => {
                                    response_application_error()
                                },
                                FunctionInvokeAndExecuteError::DefinitionMissing(_)
                                | FunctionInvokeAndExecuteError::HandlerNotDefined => response_function_misconfigured(),
                                FunctionInvokeAndExecuteError::CodeNotFound => response_service_not_found(),
                                FunctionInvokeAndExecuteError::CodeFailedToLoad(err) => {
                                    error!("failed to load function code while serving request: {err:?}");
                                    response_internal_error()
                                },
                                FunctionInvokeAndExecuteError::FailedToCompile(err) => {
                                    error!("failed to compile function while serving request: {err:?}");
                                    response_internal_error()
                                },
                                FunctionInvokeAndExecuteError::FailedToInstantiate(err) => {
                                    // TODO: not necessary runtime error, can be bad function (e.g., requiring import in fx namespace that is not implemented)
                                    error!("failed to instantiate function while serving request: {err:?}");
                                    response_internal_error()
                                }
                            }, None)
                        },
                        Err(err) => {
                            error!("timeout when serving request: {err:?}");
                            (response_internal_error(), None)
                        }
                    };

                    drop(metric_guard_http_functions_in_flight);
                    drop(body_stream_cleanup_guard);

                    (fx_response, invocation_event)
                },
                Err(err) => {
                    error!("failed to push stream: {err:?}");
                    (response_internal_error(), None)
                }
            };

            let mut response = Response::new(Full::new(Bytes::from(fx_response.body)));
            *response.status_mut() = fx_response.status;
            *response.headers_mut() = fx_response.headers;
            drop(metric_guard_http_requests_in_flight);
            drop(metric_guard_http_futures_in_flight);
            engine.metrics.http_requests_total.inc();
            engine.log(FunctionInvokeEvent {
                request_id,
                timings: InvocationTimings {
                    total_time_millis: (Instant::now() - started_at).as_millis() as u64,
                },
                compiler_backend: None,
            }.into());

            Ok(response)
        });

        Self {
            inner,
        }
    }
}

impl<'a> Future for HttpHandlerFuture<'a> {
    type Output = Result<Response<Full<Bytes>>, Infallible>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        self.inner.poll_unpin(cx)
    }
}

// for simplicity, this wrapper will cleanup stream from arena. Ideally, items in arena would have reference count.
struct StreamPoolCleanupGuard {
    engine: Arc<Engine>,
    body_stream_index: crate::runtime::streams::HostPoolIndex,
}

impl StreamPoolCleanupGuard {
    fn wrap(engine: Arc<Engine>, body_stream_index: crate::runtime::streams::HostPoolIndex) -> Self {
        Self {
            engine,
            body_stream_index,
        }
    }
}

impl Drop for StreamPoolCleanupGuard {
    fn drop(&mut self) {
        if let Err(err) = self.engine.streams_pool.remove(&self.body_stream_index) {
            error!("failed to drop body stream from streams arena: {err:?}");
        };
    }
}

struct MetricGaugeDecreaseGuard {
    gauge: prometheus::core::GenericGauge<prometheus::core::AtomicI64>,
}

impl MetricGaugeDecreaseGuard {
    fn wrap(gauge: prometheus::core::GenericGauge<prometheus::core::AtomicI64>) -> Self {
        Self { gauge }
    }
}

impl Drop for MetricGaugeDecreaseGuard {
    fn drop(&mut self) {
        self.gauge.dec();
    }
}

fn response_service_not_found() -> HttpResponse {
    HttpResponse::new().with_status(StatusCode::NOT_FOUND).with_body("fx error: service not found.\n")
}

fn response_internal_error() -> HttpResponse {
    HttpResponse::new().with_status(StatusCode::INTERNAL_SERVER_ERROR).with_body("fx: internal runtime error.\n")
}

fn response_application_error() -> HttpResponse {
    HttpResponse::new().with_status(StatusCode::BAD_GATEWAY).with_body("fx: function error.\n")
}

fn response_function_misconfigured() -> HttpResponse {
    HttpResponse::new().with_status(StatusCode::BAD_GATEWAY).with_body("fx: function misconfigured.\n")
}
