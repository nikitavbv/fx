use {
    std::{convert::Infallible, pin::Pin, time::{Instant, Duration}, sync::{Arc, Mutex}},
    tracing::{error, warn},
    hyper::{Response, body::Bytes, StatusCode},
    http_body_util::{Full, BodyStream},
    futures::{StreamExt, stream::BoxStream, future::BoxFuture, FutureExt},
    tokio::time::timeout,
    fx_common::{HttpResponse, HttpRequest, FxStream},
    fx_runtime_common::{FunctionInvokeEvent, events::InvocationTimings},
    fx_runtime::{FxRuntime, FunctionId, error::FxRuntimeError, runtime::Engine},
};

#[derive(Clone)]
pub struct HttpHandler {
    fx: FxRuntime,
    service_id: FunctionId,
}

impl HttpHandler {
    #[allow(dead_code)]
    pub fn new(fx: FxRuntime, service_id: FunctionId) -> Self {
        Self {
            fx,
            service_id,
        }
    }
}

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for HttpHandler {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        Box::pin(HttpHandlerFuture::new(self.fx.engine.clone(), self.service_id.clone(), req))
    }
}

struct HttpHandlerFuture<'a> {
    inner: BoxFuture<'a, Result<Response<Full<Bytes>>, Infallible>>,
}

impl<'a> HttpHandlerFuture<'a> {
    fn new(engine: Arc<Engine>, service_id: FunctionId, req: hyper::Request<hyper::body::Incoming>) -> Self {
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
            let body: BoxStream<'static, Vec<u8>> = BodyStream::new(req.into_body()).map(|v| v.unwrap().into_data().unwrap().to_vec()).boxed();
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
                                FxRuntimeError::ServiceNotFound => response_service_not_found(),
                                other => {
                                    error!("internal error while serving request: {other:?}");
                                    response_internal_error()
                                },
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
                compiler_backend: invocation_event.map(|v| v.compiler_metadata.backend),
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
    body_stream_index: fx_runtime::streams::HostPoolIndex,
}

impl StreamPoolCleanupGuard {
    fn wrap(engine: Arc<Engine>, body_stream_index: fx_runtime::streams::HostPoolIndex) -> Self {
        Self {
            engine,
            body_stream_index,
        }
    }
}

impl Drop for StreamPoolCleanupGuard {
    fn drop(&mut self) {
        self.engine.streams_pool.remove(&self.body_stream_index).unwrap();
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
