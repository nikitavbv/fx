use {
    std::{pin::Pin, future::Future, collections::HashMap, sync::Mutex, sync::OnceLock},
    thiserror::Error,
    serde::{de::DeserializeOwned, Serialize},
    lazy_static::lazy_static,
    futures::FutureExt,
    crate::{sys::{FunctionResourceId, FunctionResource, add_function_resource}, api::http::HttpResponse},
};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type HttpHandlerFunction = Box<dyn Fn(FunctionRequest) -> BoxFuture<FunctionResponse> + Send + Sync>;

pub struct FunctionRequest {}

impl FunctionRequest {
    pub fn into_legacy_http_request(self) -> crate::HttpRequest {
        crate::HttpRequest::new(http::Method::GET, "/".parse().unwrap())
    }
}

pub struct FunctionResponse(pub(crate) FunctionResponseInner);

impl FunctionResponse {
    pub fn into_legacy_http_response(self) -> crate::HttpResponse {
        crate::HttpResponse::new()
    }
}

pub(crate) enum FunctionResponseInner {
    HttpResponse(FunctionHttpResponse),
}

pub(crate) struct FunctionHttpResponse {
    // TODO: have http parts here too?
    pub(crate) status: http::status::StatusCode,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: FunctionResourceId,
}

pub trait IntoFunctionResponse {
    fn into_function_response(self) -> FunctionResponse;
}

impl IntoFunctionResponse for FunctionResponse {
    fn into_function_response(self) -> FunctionResponse {
        self
    }
}

impl IntoFunctionResponse for HttpResponse {
    fn into_function_response(self) -> FunctionResponse {
        let (parts, body) = self.into_parts();

        FunctionResponse(FunctionResponseInner::HttpResponse(FunctionHttpResponse {
            status: parts.status,
            headers: parts.headers,
            body: add_function_resource(FunctionResource::FunctionResponseBody(body)),
        }))
    }
}
