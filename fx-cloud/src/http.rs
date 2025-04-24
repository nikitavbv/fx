use {
    std::{convert::Infallible, pin::Pin},
    tracing::error,
    tokio::sync::oneshot,
    hyper::{Response, body::Bytes, StatusCode, header::HeaderName},
    http_body_util::Full,
    fx_core::{HttpResponse, HttpRequest},
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
        let (tx, rx) = oneshot::channel();
        let engine = self.fx.engine.clone();
        let service_id = self.service_id.clone();
        engine.clone().thread_pool.spawn(move || {
            let request = HttpRequest { url: req.uri().to_string() };
            let fx_response: HttpResponse = match engine.clone().invoke_service(engine, &service_id, "http", request) {
                Ok(v) => v,
                Err(err) => match err {
                    FxCloudError::ServiceNotFound => response_service_not_found(),
                    FxCloudError::StorageInternalError { reason: _ }
                    | FxCloudError::ServiceInternalError { reason: _ }=> {
                        error!("internal error while serving request: {err:?}");
                        response_internal_error()
                    },
                }
            };

            let mut response = Response::new(Full::new(Bytes::from(fx_response.body.unwrap_or(Vec::new()))));
            *response.status_mut() = StatusCode::from_u16(fx_response.status).unwrap();
            for (header_key, header_value) in fx_response.headers {
                response.headers_mut().append(HeaderName::from_bytes(header_key.as_bytes()).unwrap(), header_value.try_into().unwrap());
            }

            // ignore errors here, because it can be caused by client disconnecting and channel being closed
            let _res = tx.send(Ok(response));
        });

        Box::pin(async move { rx.await.unwrap() })
    }
}

fn response_service_not_found() -> HttpResponse {
    HttpResponse::new().status(404).body("fx error: service not found.\n")
}

fn response_internal_error() -> HttpResponse {
    HttpResponse::new().status(503).body("fx: internal runtime error.\n")
}
