use {
    std::{rc::Rc, cell::{RefCell, Cell}, collections::HashMap, convert::Infallible, pin::Pin, task::Poll},
    futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::{BoxStream, LocalBoxStream}, Stream},
    hyper::{Response, body::Bytes},
    http::StatusCode,
    http_body_util::BodyExt,
    send_wrapper::SendWrapper,
    crate::{
        resources::{
            serialize::{SerializeResource, SerializedFunctionResource, DeserializableResource, SerializableResource},
            ResourceId,
            resource::OwnedFunctionResourceId,
            FunctionResourceId,
        },
        function::{
            FunctionId,
            FunctionDeploymentId,
            deployment::{FunctionDeployment, FunctionDeploymentHandleRequestError},
            abi::{capnp, abi_http_capnp},
            instance::{FunctionInstance, FunctionFramePollFuture},
        },
        effects::fetch::HttpStreamError,
    },
};

const HTTP_PATH_NAMESPACE_INTERNAL: &'static str = "/_fx/";

pub(crate) struct HttpHandler {
    http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
    http_default: Rc<RefCell<Option<FunctionId>>>,
    functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<FunctionDeployment>>>>,
}

impl HttpHandler {
    pub fn new(
        http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
        http_default: Rc<RefCell<Option<FunctionId>>>,
        functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
        function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<FunctionDeployment>>>>,
    ) -> Self {
        Self {
            http_hosts,
            http_default,
            functions,
            function_deployments,
        }
    }
}

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for HttpHandler {
    type Response = Response<HttpBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn call(&self, req: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        let host_header = req.headers().get("Host");
        let host_str = match host_header {
            Some(header) => match header.to_str() {
                Ok(v) => Some(v),
                Err(_) => {
                    return Box::pin(async move {
                        let mut response = Response::new(HttpBody::for_bytes(Bytes::from("invalid Host header\n".as_bytes())));
                        *response.status_mut() = StatusCode::BAD_REQUEST;
                        Ok(response)
                    });
                }
            },
            None => None,
        };

        let target_function = host_str
            .and_then(|v| self.http_hosts.borrow().get(&v.to_lowercase()).cloned())
            .or_else(|| self.http_default.borrow().clone());
        let target_function_deployment_id = target_function.and_then(|function_id| self.functions.borrow().get(&function_id).cloned());
        let target_function_deployment = target_function_deployment_id.and_then(|instance_id| self.function_deployments.borrow().get(&instance_id).cloned());

        Box::pin(async move {
            let target_function_deployment = match target_function_deployment {
                Some(v) => v,
                None => {
                    let mut response = Response::new(HttpBody::for_bytes(Bytes::from("no fx function found to handle this request.\n".as_bytes())));
                    *response.status_mut() = StatusCode::BAD_GATEWAY;
                    return Ok(response);
                }
            };

            let (header, body) = req.into_parts();

            if header.uri.path().starts_with(HTTP_PATH_NAMESPACE_INTERNAL) {
                let mut response = Response::new(HttpBody::for_bytes(Bytes::from("not found.\n".as_bytes())));
                *response.status_mut() = StatusCode::NOT_FOUND;
                return Ok(response);
            }

            let function_future = target_function_deployment
                .handle_request(
                    FetchRequestHeader::from(header),
                    Some(HttpBody::for_stream(body.into_data_stream().map(|v| v.map_err(|_| todo!())).boxed()))
                ).await;
            let response = function_future.await;
            let function_response = match response {
                Ok(v) => Ok(v.move_to_host().await),
                Err(err) => Err(err),
            };

            let body = match &function_response {
                Ok(response) => match &response.as_ref().unwrap().0 {
                    FunctionResponseInner::HttpResponse(v) => {
                        let (instance, body_resource_id) = v.body.replace(None).unwrap().consume();
                        DeserializableResource::from_serialized(SerializedFunctionResource::<HttpBody>::new(instance, body_resource_id))
                    }
                },
                Err(err) => match err {
                    FunctionDeploymentHandleRequestError::FunctionPanicked => DeserializableResource::Raw(HttpBody::for_bytes(Bytes::from("function panicked while handling request.\n")))
                }
            };

            let mut response = Response::new(body.copy_to_host().await.unwrap());
            match function_response {
                Ok(function_response) => match function_response.unwrap().0 {
                    FunctionResponseInner::HttpResponse(v) => {
                        *response.status_mut() = v.status;
                        *response.headers_mut() = v.headers;
                    }
                },
                Err(err) => match err {
                    FunctionDeploymentHandleRequestError::FunctionPanicked => {
                        *response.status_mut() = StatusCode::BAD_GATEWAY;
                    }
                }
            }

            Ok(response)
        })
    }
}

pub(crate) struct HttpBody(pub(crate) HttpBodyInner);

impl HttpBody {
    pub fn for_bytes(bytes: Bytes) -> Self {
        Self::for_stream(futures::stream::once(async { Ok(bytes) }).boxed())
    }

    pub fn for_function_stream(resource: OwnedFunctionResourceId) -> Self {
        Self(HttpBodyInner::FunctionStream(SendWrapper::new(RefCell::new(Some(FunctionStreamReader::new(resource))))))
    }

    pub fn for_stream(stream: BoxStream<'static, Result<Bytes, HttpStreamError>>) -> Self {
        Self(HttpBodyInner::Stream { stream, frame: None })
    }
}

impl hyper::body::Body for HttpBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match &mut self.0 {
            HttpBodyInner::FunctionStream(resource) =>
                resource.borrow_mut().as_mut().unwrap()
                    .poll_frame(cx)
                    .map(|v| v.map(|v| Ok(hyper::body::Frame::data(Bytes::from(v))))),
            HttpBodyInner::Stream { stream, frame: _previous_frame_is_discarded } => stream.poll_next_unpin(cx)
                .map(|v| v.map(|v|
                    v
                        .map(hyper::body::Frame::data)
                        .map_err(std::io::Error::other)
                )),
            HttpBodyInner::StreamLocal { stream, frame: _previous_frame_is_discarded } => stream.poll_next_unpin(cx)
                .map(|v| v.map(|v|
                    v
                        .map(hyper::body::Frame::data)
                        .map_err(std::io::Error::other)
                )),
        }
    }
}

pub(crate) enum HttpBodyInner {
    FunctionStream(SendWrapper<RefCell<Option<FunctionStreamReader>>>),
    Stream {
        stream: BoxStream<'static, Result<Bytes, HttpStreamError>>,
        frame: Option<SerializableResource<Option<Result<Bytes, HttpStreamError>>>>,
    },
    StreamLocal {
        stream: SendWrapper<LocalBoxStream<'static, Result<Bytes, HttpStreamError>>>,
        frame: Option<SerializableResource<Option<Result<Bytes, HttpStreamError>>>>,
    },
}

pub(crate) struct FunctionStreamReader {
    function: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
    poll_future: Option<LocalBoxFuture<'static, Option<Vec<u8>>>>,
}

impl FunctionStreamReader {
    fn new(resource: OwnedFunctionResourceId) -> Self {
        let (function, resource_id) = resource.consume();

        Self {
            function,
            resource_id,
            poll_future: None,
        }
    }

    fn poll_frame(&mut self, context: &mut std::task::Context<'_>) -> Poll<Option<Vec<u8>>> {
        let mut poll_future = match self.poll_future.take() {
            Some(v) => v,
            None => {
                let function = self.function.clone();
                let resource_id = self.resource_id.clone();

                async move {
                    FunctionFramePollFuture::new(function.clone(), resource_id.clone()).await;
                    let result = function.stream_frame_read_v2(&resource_id).await;
                    function.stream_advance(&resource_id).await;
                    result
                }.boxed_local()
            },
        };

        let result = poll_future.poll_unpin(context);
        if result.is_pending() {
            self.poll_future = Some(poll_future);
        }

        result
    }
}

impl Stream for FunctionStreamReader {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_frame(cx)
    }
}

pub struct FetchRequestHeader {
    inner: ::http::request::Parts,
    pub(crate) body_resource_id: Option<ResourceId>, // TODO: drop body if FetchRequestHeader is dropped without consumption
}

impl FetchRequestHeader {
    pub(crate) fn from_http_parts(inner: ::http::request::Parts) -> Self {
        Self {
            inner,
            body_resource_id: None,
        }
    }

    pub(crate) fn uri(&self) -> &::http::Uri {
        &self.inner.uri
    }

    pub(crate) fn method(&self) -> &::http::Method {
        &self.inner.method
    }

    pub(crate) fn headers(&self) -> &::http::HeaderMap {
        &self.inner.headers
    }
}

impl From<::http::request::Parts> for FetchRequestHeader {
    fn from(value: ::http::request::Parts) -> Self {
        Self {
            inner: value,
            body_resource_id: None,
        }
    }
}

pub(crate) struct FunctionResponse(pub(crate) FunctionResponseInner);

pub(crate) enum FunctionResponseInner {
    HttpResponse(FunctionHttpResponse),
}

pub(crate) struct FunctionHttpResponse {
    pub(crate) status: ::http::status::StatusCode,
    pub(crate) headers: ::http::HeaderMap,
    pub(crate) body: Cell<Option<OwnedFunctionResourceId>>,
}
