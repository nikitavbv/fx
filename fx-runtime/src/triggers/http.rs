use {
    std::{rc::Rc, cell::{RefCell, Cell}, collections::HashMap, convert::Infallible, pin::Pin, task::Poll},
    futures::{FutureExt, future::LocalBoxFuture, stream::BoxStream},
    hyper::{Response, body::Bytes},
    http::StatusCode,
    send_wrapper::SendWrapper,
    crate::{
        resources::{
            serialize::{SerializeResource, SerializedFunctionResource},
            ResourceId,
            resource::OwnedFunctionResourceId,
            FunctionResourceId,
        },
        function::{
            FunctionId,
            FunctionDeploymentId,
            deployment::{FunctionDeployment, FunctionDeploymentHandleRequestError},
            abi::{capnp, abi_http_capnp},
            instance::FunctionInstance,
        },
        effects::fetch::HttpStreamFrame,
    },
};

pub(crate) struct HttpHandler {
    http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
    http_default: Rc<RefCell<Option<FunctionId>>>,
    functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>>,
}

impl HttpHandler {
    pub fn new(
        http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
        http_default: Rc<RefCell<Option<FunctionId>>>,
        functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
        function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>>,
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
        let target_function = req.headers().get("Host")
            .and_then(|v| self.http_hosts.borrow().get(&v.to_str().unwrap().to_lowercase()).cloned())
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

            let function_future = target_function_deployment.borrow().handle_request(FetchRequestHeader::from(header), Some(FetchRequestBody::from(body)));
            let response = function_future.await;
            let function_response = match response {
                Ok(v) => Ok(v.move_to_host().await),
                Err(err) => Err(err),
            };

            let body = match &function_response {
                Ok(response) => match &response.0 {
                    FunctionResponseInner::HttpResponse(v) => {
                        HttpBody::for_function_resource(v.body.replace(None).unwrap())
                    }
                },
                Err(err) => match err {
                    FunctionDeploymentHandleRequestError::FunctionPanicked => HttpBody::for_bytes(Bytes::from("function panicked while handling request.\n"))
                }
            };

            let mut response = Response::new(body);
            match function_response {
                Ok(function_response) => match function_response.0 {
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
        Self(HttpBodyInner::Full(SendWrapper::new(RefCell::new(Some(bytes)))))
    }

    pub fn for_function_resource(resource: OwnedFunctionResourceId) -> Self {
        Self(HttpBodyInner::FunctionResource(SendWrapper::new(RefCell::new(FunctionResourceReader::Resource(resource)))))
    }

    pub fn for_stream(stream: BoxStream<'static, Result<Bytes, ()>>) -> Self {
        Self(HttpBodyInner::Stream(stream))
    }
}

impl hyper::body::Body for HttpBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match &self.0 {
            HttpBodyInner::Empty => Poll::Ready(None),
            HttpBodyInner::Full(b) => Poll::Ready(b.replace(None).map(|v| Ok(hyper::body::Frame::data(v)))),
            HttpBodyInner::FunctionResource(resource) => {
                let mut reader = resource.replace(FunctionResourceReader::Empty);

                // if finished reading, then finish this body
                reader = match reader {
                    FunctionResourceReader::Empty => return Poll::Ready(None),
                    other => other,
                };

                // if there is a HttpBody resource we can start reading, let's request next frame
                reader = match reader {
                    FunctionResourceReader::Resource(resource_id) => {
                        let (instance, resource_id) = resource_id.consume();

                        let mut next_frame_poll_future = {
                            let resource_id = resource_id.clone();
                            let instance = instance.clone();

                            async move {
                                let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
                                instance.future_poll(&resource_id, waker).await.unwrap()
                            }.boxed_local()
                        };

                        match next_frame_poll_future.poll_unpin(cx) {
                            Poll::Pending => {
                                resource.replace(FunctionResourceReader::FramePollFuture {
                                    instance,
                                    resource_id,
                                    future: next_frame_poll_future,
                                });
                                return Poll::Pending;
                            },
                            Poll::Ready(Poll::Pending) => {
                                resource.replace(FunctionResourceReader::Resource(OwnedFunctionResourceId::new(instance, resource_id)));
                                return Poll::Pending;
                            },
                            Poll::Ready(Poll::Ready(_)) => FunctionResourceReader::FrameReadFuture {
                                instance: instance.clone(),
                                resource_id: resource_id.clone(),
                                future: async move {
                                    instance.stream_frame_read(&resource_id).await
                                }.boxed_local(),
                            },
                        }
                    },
                    other => other,
                };

                // if there is a frame we are currently reading, poll that
                let (reader, poll_result) = match reader {
                    FunctionResourceReader::FrameReadFuture { instance, resource_id, mut future } => match future.poll_unpin(cx) {
                        Poll::Pending => todo!(),
                        Poll::Ready(None) => return Poll::Ready(None),
                        Poll::Ready(Some(HttpStreamFrame::Bytes(bytes))) => {
                            (FunctionResourceReader::Resource(OwnedFunctionResourceId::new(instance, resource_id)), Poll::Ready(Some(Ok(hyper::body::Frame::data(Bytes::from(bytes))))))
                        },
                    },
                    _ => unreachable!("didn't expect to get this reader state"),
                };

                resource.replace(reader);

                poll_result
            },
            HttpBodyInner::Stream(_) => todo!(),
            HttpBodyInner::StreamPartiallyRead { stream, frame } => todo!(), // TODO: can be partially read by both function and host?
            HttpBodyInner::StreamPartiallyReadSerialized { stream, frame_serialized } => todo!(), // TODO: can be partially read by both function and host?
        }
    }
}

pub(crate) enum HttpBodyInner {
    Empty,
    Full(SendWrapper<RefCell<Option<Bytes>>>),
    FunctionResource(SendWrapper<RefCell<FunctionResourceReader>>),
    Stream(BoxStream<'static, Result<Bytes, ()>>),
    StreamPartiallyRead {
        stream: BoxStream<'static, Result<Bytes, ()>>,
        frame: Result<Bytes, ()>,
    },
    StreamPartiallyReadSerialized {
        stream: BoxStream<'static, Result<Bytes, ()>>,
        frame_serialized: Vec<u8>,
    },
}

pub(crate) enum FunctionResourceReader {
    // HttpBody is empty or we finished reading it
    Empty,
    // HttpBody is a resource on function side, no frame is in progress of being read
    Resource(OwnedFunctionResourceId),
    // polling next frame of HttpBody stream
    FramePollFuture {
        instance: Rc<FunctionInstance>,
        resource_id: FunctionResourceId,
        future: LocalBoxFuture<'static, Poll<()>>,
    },
    // frame is ready, reading it now
    FrameReadFuture {
        instance: Rc<FunctionInstance>,
        resource_id: FunctionResourceId,
        future: LocalBoxFuture<'static, Option<HttpStreamFrame>>,
    },
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

    fn uri(&self) -> &::http::Uri {
        &self.inner.uri
    }

    fn method(&self) -> &::http::Method {
        &self.inner.method
    }

    fn headers(&self) -> &::http::HeaderMap {
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

pub struct FetchRequestBody(pub(crate) FetchRequestBodyInner);

impl From<hyper::body::Incoming> for FetchRequestBody {
    fn from(value: hyper::body::Incoming) -> Self {
        Self(FetchRequestBodyInner::Stream(Box::pin(value)))
    }
}

// TODO: can use HttpBody everywhere?
pub(crate) enum FetchRequestBodyInner {
    // full body:
    Full(Vec<u8>),
    FullSerialized(Vec<u8>),
    // streaming body:
    Stream(Pin<Box<hyper::body::Incoming>>),
    PartiallyReadStream {
        stream: Pin<Box<hyper::body::Incoming>>,
        frame: Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>, // TODO: why PartiallyReadStream exists? can't we just go from Stream to PartiallyReadStreamSerialized?
    },
    PartiallyReadStreamSerialized {
        stream: Pin<Box<hyper::body::Incoming>>,
        frame_serialized: Vec<u8>,
    },
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

impl SerializeResource for FetchRequestHeader {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut resource = message.init_root::<abi_http_capnp::http_request::Builder>();

        resource.set_uri(self.uri().to_string());
        resource.set_method(match &*self.method() {
            &hyper::Method::GET => abi_http_capnp::HttpMethod::Get,
            &hyper::Method::POST => abi_http_capnp::HttpMethod::Post,
            &hyper::Method::PUT => abi_http_capnp::HttpMethod::Put,
            &hyper::Method::PATCH => abi_http_capnp::HttpMethod::Patch,
            &hyper::Method::DELETE => abi_http_capnp::HttpMethod::Delete,
            &hyper::Method::OPTIONS => abi_http_capnp::HttpMethod::Options,
            other => todo!("this http method not supported: {other:?}"),
        });

        let mut request_headers = resource.reborrow().init_headers(self.headers().len() as u32);
        for (index, (header_name, header_value)) in self.headers().iter().enumerate() {
            let mut request_header = request_headers.reborrow().get(index as u32);
            request_header.set_name(header_name.as_str());
            request_header.set_value(header_value.to_str().unwrap());
        }

        let mut resource_body = resource.init_body().init_body();
        match self.body_resource_id {
            None => resource_body.set_empty(()),
            Some(resource_id) => resource_body.set_host_resource(resource_id.as_u64()),
        }

        capnp::serialize::write_message_to_words(&message)
    }
}
