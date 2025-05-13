use {
    std::{convert::Infallible, pin::Pin},
    tracing::error,
    hyper::{Response, body::Bytes, StatusCode},
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
        let engine = self.fx.engine.clone();
        let service_id = self.service_id.clone();
        Box::pin(async move {
            let request = HttpRequest {
                url: req.uri().to_string(),
                headers: req.headers().clone(),
            };
            let fx_response: HttpResponse = match engine.clone().invoke_service(engine, &service_id, "http", request).await {
                Ok(v) => v,
                Err(err) => match err {
                    FxCloudError::ServiceNotFound => response_service_not_found(),
                    FxCloudError::StorageInternalError { reason: _ }
                    | FxCloudError::ServiceInternalError { reason: _ }
                    | FxCloudError::CompilationError { reason: _ }
                    | FxCloudError::RpcHandlerNotDefined
                    | FxCloudError::RpcHandlerIncompatibleType
                    | FxCloudError::ModuleCodeNotFound
                    | FxCloudError::ConfigurationError { reason: _ } => {
                        error!("internal error while serving request: {err:?}");
                        response_internal_error()
                    },
                }
            };

            let mut response = Response::new(Full::new(Bytes::from(fx_response.body)));
            *response.status_mut() = fx_response.status;
            *response.headers_mut() = fx_response.headers;
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
