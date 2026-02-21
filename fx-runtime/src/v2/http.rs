use {
    std::{rc::Rc, cell::RefCell, collections::HashMap, convert::Infallible, pin::Pin, task::Poll},
    futures::FutureExt,
    crate::v2::{
        FunctionId,
        function::{FunctionDeploymentId, FunctionDeployment},
        Response,
        Bytes,
        StatusCode,
        FetchRequestHeader,
        FetchRequestBody,
        FunctionResponseInner,
        FunctionDeploymentHandleRequestError,
        FunctionResponseHttpBodyInner,
        SerializedFunctionResource,
        FunctionResourceReader,
    },
};

pub(crate) struct HttpHandlerV2 {
    http_hosts: Rc<RefCell<HashMap<String, FunctionId>>>,
    http_default: Rc<RefCell<Option<FunctionId>>>,
    functions: Rc<RefCell<HashMap<FunctionId, FunctionDeploymentId>>>,
    function_deployments: Rc<RefCell<HashMap<FunctionDeploymentId, Rc<RefCell<FunctionDeployment>>>>>,
}

impl HttpHandlerV2 {
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

impl hyper::service::Service<hyper::Request<hyper::body::Incoming>> for HttpHandlerV2 {
    type Response = Response<FunctionResponseHttpBody>;
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
                    let mut response = Response::new(FunctionResponseHttpBody::for_bytes(Bytes::from("no fx function found to handle this request.\n".as_bytes())));
                    *response.status_mut() = StatusCode::BAD_GATEWAY;
                    return Ok(response);
                }
            };

            let (header, body) = req.into_parts();

            let function_future = target_function_deployment.borrow().handle_request(FetchRequestHeader::from(header), FetchRequestBody::from(body));
            let response = function_future.await;
            let function_response = match response {
                Ok(v) => Ok(v.move_to_host().await),
                Err(err) => Err(err),
            };

            let body = match &function_response {
                Ok(response) => match &response.0 {
                    FunctionResponseInner::HttpResponse(v) => {
                        FunctionResponseHttpBody::for_function_resource(v.body.replace(None).unwrap())
                    }
                },
                Err(err) => match err {
                    FunctionDeploymentHandleRequestError::FunctionPanicked => FunctionResponseHttpBody::for_bytes(Bytes::from("function panicked while handling request.\n"))
                }
            };

            let mut response = Response::new(body);
            match function_response {
                Ok(function_response) => match &function_response.0 {
                    FunctionResponseInner::HttpResponse(v) => {
                        *response.status_mut() = v.status;
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

pub(crate) struct FunctionResponseHttpBody(FunctionResponseHttpBodyInner);

impl FunctionResponseHttpBody {
    pub fn for_bytes(bytes: Bytes) -> Self {
        Self(FunctionResponseHttpBodyInner::Full(RefCell::new(Some(bytes))))
    }

    pub fn for_function_resource(resource: SerializedFunctionResource<Vec<u8>>) -> Self {
        Self(FunctionResponseHttpBodyInner::FunctionResource(RefCell::new(FunctionResourceReader::Resource(resource))))
    }
}

impl hyper::body::Body for FunctionResponseHttpBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match &self.0 {
            FunctionResponseHttpBodyInner::Full(b) => return Poll::Ready(b.replace(None).map(|v| Ok(hyper::body::Frame::data(v)))),
            FunctionResponseHttpBodyInner::FunctionResource(resource) => {
                let reader = resource.replace(FunctionResourceReader::Empty);

                let mut reader = match reader {
                    FunctionResourceReader::Empty => return Poll::Ready(None),
                    FunctionResourceReader::Future(v) => FunctionResourceReader::Future(v),
                    FunctionResourceReader::Resource(v) => {
                        FunctionResourceReader::Future(async move {
                            v.move_to_host().await
                        }.boxed_local())
                    }
                };

                let poll_result = match &mut reader {
                    FunctionResourceReader::Empty | FunctionResourceReader::Resource(_) => unreachable!(),
                    FunctionResourceReader::Future(v) => v.poll_unpin(cx).map(|v| Some(Ok(hyper::body::Frame::data(Bytes::from(v))))),
                };

                if poll_result.is_pending() {
                    resource.replace(reader);
                }

                poll_result
            },
        }
    }
}
