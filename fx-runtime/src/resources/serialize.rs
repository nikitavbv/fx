use {
    std::{marker::PhantomData, rc::Rc, cell::Cell},
    thiserror::Error,
    crate::{
        function::{
            abi::{capnp, abi_http_capnp},
            instance::FunctionInstance,
        },
        triggers::http::{FunctionResponse, FunctionResponseInner, FunctionHttpResponse},
        resources::{resource::OwnedFunctionResourceId, FunctionResourceId},
    },
};

pub(crate) trait SerializeResource {
    fn serialize(self) -> Vec<u8>;
}

pub(crate) enum SerializableResource<T: SerializeResource> {
    Raw(T),
    Serialized(Vec<u8>),
}

impl<T: SerializeResource> SerializableResource<T> {
    pub(crate) fn map_to_serialized(self) -> Self {
        match self {
            Self::Raw(t) => Self::Serialized(t.serialize()),
            Self::Serialized(v) => Self::Serialized(v),
        }
    }

    pub(crate) fn serialized_size(&self) -> usize {
        match self {
            Self::Raw(_) => panic!("cannot compute serialized size for resource that is not serialized yet"),
            Self::Serialized(v) => v.len(),
        }
    }

    pub(crate) fn into_serialized(self) -> Vec<u8> {
        match self {
            Self::Raw(t) => t.serialize(),
            Self::Serialized(v) => v,
        }
    }
}

impl SerializeResource for Vec<u8> {
    fn serialize(self) -> Vec<u8> {
        self
    }
}

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

    pub(crate) async fn copy_to_host(self) -> Result<T, <T as DeserializeFunctionResource>::Error> {
        let (instance, resource) = self.resource.consume();
        T::deserialize(&mut instance.copy_serializable_resource_to_host(&resource).await.as_slice(), instance)
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

impl DeserializeFunctionResource for FunctionResponse {
    type Error = DeserializationError;

    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Result<Self, Self::Error> {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let response = message_reader.get_root::<abi_http_capnp::function_response::Reader>().unwrap();

        let mut headers = ::http::HeaderMap::new();
        for header in response.get_headers().unwrap() {
            let name = ::http::HeaderName::from_bytes(header.get_name().unwrap().as_bytes()).unwrap();
            let value = ::http::HeaderValue::from_str(header.get_value().unwrap().to_str().unwrap()).unwrap();
            headers.insert(name, value);
        }

        Ok(Self(FunctionResponseInner::HttpResponse(FunctionHttpResponse {
            status: ::http::StatusCode::from_u16(response.get_status()).unwrap(),
            headers,
            body: Cell::new(Some(OwnedFunctionResourceId::new(instance, FunctionResourceId::from(response.get_body_resource())))),
        })))
    }
}

impl DeserializeFunctionResource for Vec<u8> {
    type Error = ();

    fn deserialize(resource: &mut &[u8], _instance: Rc<FunctionInstance>) -> Result<Self, Self::Error> {
        Ok(resource.to_vec())
    }
}

pub(crate) enum DeserializableResource<T: DeserializeFunctionResource> {
    Raw(T),
    Serialized(SerializedFunctionResource<T>),
}

impl<T: DeserializeFunctionResource> DeserializableResource<T> {
    pub fn from_serialized(value: SerializedFunctionResource<T>) -> Self {
        Self::Serialized(value)
    }

    pub async fn copy_to_host(self) -> Result<T, <T as DeserializeFunctionResource>::Error> {
        match self {
            Self::Raw(v) => Ok(v),
            Self::Serialized(v) => v.copy_to_host().await,
        }
    }
}
