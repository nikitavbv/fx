use {
    std::rc::Rc,
    futures::future::{LocalBoxFuture, FutureExt},
    fx_types::{capnp, abi_http_capnp},
    crate::{
        resources::{FunctionResourceId, resource::OwnedFunctionResourceId},
        triggers::http::HttpBody,
        function::instance::FunctionInstance,
    },
};

pub(crate) struct FunctionHttpResponseFuture {
    inner: LocalBoxFuture<'static, http::Response<HttpBody>>,
}

impl FunctionHttpResponseFuture {
    pub fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: async move {
                let resource = instance.move_serializable_resource_to_host(&resource_id).await;
                let message_reader = capnp::serialize::read_message_from_flat_slice(&mut resource.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                let response = message_reader.get_root::<abi_http_capnp::http_response::Reader>().unwrap();

                let mut http_response = http::Response::builder()
                    .status(::http::StatusCode::from_u16(response.get_status()).unwrap());

                for header in response.get_headers().unwrap() {
                    let name = ::http::HeaderName::from_bytes(header.get_name().unwrap().as_bytes()).unwrap();
                    let value = ::http::HeaderValue::from_str(header.get_value().unwrap().to_str().unwrap()).unwrap();
                    http_response = http_response.header(name, value);
                }

                match response.get_body().which().unwrap() {
                    abi_http_capnp::http_response::body::Which::FunctionResourceId(resource_id) => http_response.body(
                        HttpBody::for_function_stream(OwnedFunctionResourceId::new(instance, FunctionResourceId::from(resource_id)))
                    ).unwrap(),
                    abi_http_capnp::http_response::body::Which::HostResourceId(resource_id) => http_response.body({
                        instance.clone().store.lock().await.data_mut().resource_set.http_bodies.remove(resource_id.into()).unwrap()
                    }).unwrap(),
                }
            }.boxed_local()
        }
    }
}

impl Future for FunctionHttpResponseFuture {
    type Output = http::Response<HttpBody>;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        self.inner.poll_unpin(cx)
    }
}
