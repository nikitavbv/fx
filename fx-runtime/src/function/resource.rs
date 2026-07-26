use {
    std::rc::Rc,
    tracing::warn,
    futures::future::{LocalBoxFuture, FutureExt},
    thiserror::Error,
    fx_types::{capnp, abi_http_capnp},
    crate::{
        resources::FunctionResourceId,
        triggers::http::HttpBody,
        function::instance::FunctionInstance,
    },
};

pub(crate) struct FunctionHttpResponseFuture {
    inner: LocalBoxFuture<'static, Result<http::Response<HttpBody>, FunctionHttpResponseFutureError>>,
}

#[derive(Debug, Error)]
pub(crate) enum FunctionHttpResponseFutureError {
    #[error("host resource referenced by function is not found")]
    HostResourceNotFound,
}

impl FunctionHttpResponseFuture {
    pub fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: async move {
                let resource = instance.move_serializable_resource_to_host(&resource_id).await;
                let message_reader = capnp::serialize::read_message_from_flat_slice(&mut resource.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                let response = message_reader.get_root::<abi_http_capnp::http_response::Reader>().unwrap();

                let body = match response.get_body().which().unwrap() {
                    abi_http_capnp::http_response::body::Which::FunctionResourceId(resource_id) => HttpBody::for_function_stream(instance, resource_id.into()),
                    abi_http_capnp::http_response::body::Which::HostResourceId(resource_id) => instance.clone().store.lock().await.data_mut().resource_set.http_bodies.remove(resource_id.into())
                        .ok_or(FunctionHttpResponseFutureError::HostResourceNotFound)?,
                };

                let mut http_response = http::Response::new(body);
                *http_response.status_mut() = ::http::StatusCode::from_u16(response.get_status()).unwrap();

                for header in response.get_headers().unwrap() {
                    let name = match header.get_name() {
                        Ok(v) => v,
                        Err(err) => {
                            warn!("failed to read header name passed from function: {err:?}, skipping this header");
                            continue;
                        }
                    };
                    let name = match ::http::HeaderName::from_bytes(name.as_bytes()) {
                        Ok(v) => v,
                        Err(err) => {
                            warn!("failed to convert header name passed from function: {err:?}, skipping this header");
                            continue;
                        }
                    };

                    let value = match header.get_value() {
                        Ok(v) => v,
                        Err(err) => {
                            warn!("failed to read header value passed from function: {err:?}, skipping this header");
                            continue;
                        }
                    };
                    let value = match ::http::HeaderValue::from_bytes(value.as_bytes()) {
                        Ok(v) => v,
                        Err(err) => {
                            warn!("failed to convert header value passed from function: {err:?}, skipping this header");
                            continue;
                        },
                    };
                    http_response.headers_mut().insert(name, value);
                }

                Ok(http_response)
            }.boxed_local()
        }
    }
}

impl Future for FunctionHttpResponseFuture {
    type Output = Result<http::Response<HttpBody>, FunctionHttpResponseFutureError>;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        self.inner.poll_unpin(cx)
    }
}

#[derive(Clone)]
pub(crate) struct FunctionStreamResourceId {
    id: u64,
}

impl FunctionStreamResourceId {
    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<u64> for FunctionStreamResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}
