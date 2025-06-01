use {
    std::{convert::Infallible, pin::Pin},
    tracing::error,
    hyper::{Response, body::Bytes, StatusCode},
    http_body_util::{Full, BodyStream},
    futures::{StreamExt, stream::BoxStream},
    fx_core::{HttpResponse, HttpRequest, FxStream},
    crate::{FxCloud, ServiceId, error::FxCloudError},
};

#[derive(Clone)]
pub struct HttpHandler {
    fx: FxCloud,
    service_id: ServiceId,
}

impl HttpHandler {
    pub fn new(fx: FxCloud, service_id: ServiceId) -> Self {
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
        let engine = self.fx.engine.clone();
        engine.metrics.http_requests_in_flight.inc();

        let service_id = self.service_id.clone();
        Box::pin(async move {
            let method = req.method().to_owned();
            let url = req.uri().clone();
            let headers = req.headers().clone();
            let body: BoxStream<'static, Vec<u8>> = BodyStream::new(req.into_body()).map(|v| v.unwrap().into_data().unwrap().to_vec()).boxed();
            let body_stream_index = engine.streams_pool.push(body);
            let body = FxStream { index: body_stream_index.0 as i64 };

            let request = HttpRequest {
                method,
                url,
                headers,
                body: Some(body),
            };
            let fx_response: HttpResponse = match engine.invoke_service(engine.clone(), &service_id, "http", request).await {
                Ok(v) => v,
                Err(err) => match err {
                    FxCloudError::ServiceNotFound => response_service_not_found(),
                    other => {
                        error!("internal error while serving request: {other:?}");
                        response_internal_error()
                    },
                }
            };
            engine.streams_pool.remove(&body_stream_index);

            let mut response = Response::new(Full::new(Bytes::from(fx_response.body)));
            *response.status_mut() = fx_response.status;
            *response.headers_mut() = fx_response.headers;
            engine.metrics.http_requests_in_flight.dec();
            engine.metrics.http_requests_total.inc();
            Ok(response)
        })
    }
}

fn response_service_not_found() -> HttpResponse {
    HttpResponse::new().with_status(StatusCode::NOT_FOUND).with_body("fx error: service not found.\n")
}

fn response_internal_error() -> HttpResponse {
    HttpResponse::new().with_status(StatusCode::INTERNAL_SERVER_ERROR).with_body("fx: internal runtime error.\n")
}
