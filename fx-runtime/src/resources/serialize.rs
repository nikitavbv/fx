use {
    std::{marker::PhantomData, rc::Rc},
    thiserror::Error,
    crate::{
        function::{
            abi::{capnp, abi_http_capnp},
            instance::FunctionInstance,
        },
        triggers::http::HttpBody,
        resources::{resource::OwnedFunctionResourceId, FunctionResourceId},
    },
};

/// Resource that origins from function side and is not owned by host.
/// moved lazily from function to host memory.
/// if dropped before being moved, cleans up resource on function side.
pub(crate) struct SerializedFunctionResource<T: DeserializeFunctionResource> {
    _t: PhantomData<T>,
    resource: OwnedFunctionResourceId,
}

impl<T: DeserializeFunctionResource> SerializedFunctionResource<T> {
    pub fn new(instance: Rc<FunctionInstance>, resource: FunctionResourceId) -> Self {
        Self {
            _t: PhantomData,
            resource: OwnedFunctionResourceId::new(instance, resource),
        }
    }

    pub(crate) async fn move_to_host(self) -> Result<T, <T as DeserializeFunctionResource>::Error> {
        let (instance, resource) = self.resource.consume();
        T::deserialize(&mut instance.move_serializable_resource_to_host(&resource).await.as_slice(), instance)
    }
}

pub(crate) trait DeserializeFunctionResource {
    // having associated type here allows to have custom errors for different types (for example, additional validation while
    // deserializing or errors while resolving dependencies)
    type Error;

    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Result<Self, Self::Error> where Self: Sized;
}

#[derive(Debug, Error)]
pub(crate) enum DeserializationError {
}

impl DeserializeFunctionResource for http::Response<HttpBody> {
    type Error = DeserializationError;

    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Result<Self, Self::Error> {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let response = message_reader.get_root::<abi_http_capnp::http_response::Reader>().unwrap();

        let mut http_response = http::Response::builder()
            .status(::http::StatusCode::from_u16(response.get_status()).unwrap());

        for header in response.get_headers().unwrap() {
            let name = ::http::HeaderName::from_bytes(header.get_name().unwrap().as_bytes()).unwrap();
            let value = ::http::HeaderValue::from_str(header.get_value().unwrap().to_str().unwrap()).unwrap();
            http_response = http_response.header(name, value);
        }


        Ok(
            http_response.body(crate::triggers::http::HttpBody::for_function_stream(OwnedFunctionResourceId::new(instance, FunctionResourceId::from(response.get_body_resource_id()))))
                .unwrap()
        )
    }
}

impl DeserializeFunctionResource for Vec<u8> {
    type Error = ();

    fn deserialize(resource: &mut &[u8], _instance: Rc<FunctionInstance>) -> Result<Self, Self::Error> {
        Ok(resource.to_vec())
    }
}
