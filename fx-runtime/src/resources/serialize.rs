use {
    std::{marker::PhantomData, rc::Rc, cell::Cell},
    hyper::body::Bytes,
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

pub(crate) fn serialize_request_body_full(body: Vec<u8>) -> Vec<u8> {
    todo!("serialize request body full")
}

pub(crate) fn serialize_partially_read_stream(frame: Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    let serialized_frame = message.init_root::<abi_http_capnp::http_body::Builder>();
    let mut serialized_frame = serialized_frame.init_body();

    match frame {
        None => serialized_frame.set_empty(()),
        Some(Err(err)) => todo!("handle error: {err:?}"),
        Some(Ok(frame)) => serialized_frame.set_bytes(&frame.into_data().unwrap()),
    }

    capnp::serialize::write_message_to_words(&message)
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

    pub(crate) async fn copy_to_host(self) -> T {
        let (instance, resource) = self.resource.consume();
        T::deserialize(&mut instance.copy_serializable_resource_to_host(&resource).await.as_slice(), instance)
    }

    pub(crate) async fn move_to_host(self) -> T {
        let (instance, resource) = self.resource.consume();
        T::deserialize(&mut instance.move_serializable_resource_to_host(&resource).await.as_slice(), instance)
    }
}

pub(crate) trait DeserializeFunctionResource {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Result<Self, DeserializationError>;
}

#[derive(Debug, Error)]
enum DeserializationError {
    #[error("deserialization failed because of an issue with a resource this resource depends on: {message:?}")]
    DependencyError {
        message: String,
    },
}

impl DeserializeFunctionResource for FunctionResponse {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let response = message_reader.get_root::<abi_http_capnp::function_response::Reader>().unwrap();

        let mut headers = ::http::HeaderMap::new();
        for header in response.get_headers().unwrap() {
            let name = ::http::HeaderName::from_bytes(header.get_name().unwrap().as_bytes()).unwrap();
            let value = ::http::HeaderValue::from_str(header.get_value().unwrap().to_str().unwrap()).unwrap();
            headers.insert(name, value);
        }

        Self(FunctionResponseInner::HttpResponse(FunctionHttpResponse {
            status: ::http::StatusCode::from_u16(response.get_status()).unwrap(),
            headers,
            body: Cell::new(Some(OwnedFunctionResourceId::new(instance, FunctionResourceId::from(response.get_body_resource())))),
        }))
    }
}

impl DeserializeFunctionResource for Vec<u8> {
    fn deserialize(resource: &mut &[u8], _instance: Rc<FunctionInstance>) -> Self {
        resource.to_vec()
    }
}

pub(crate) enum DeserializableResource<T: DeserializeFunctionResource> {
    Raw(T),
    Serialized(SerializedFunctionResource<T>),
}

impl<T: DeserializeFunctionResource> DeserializableResource<T> {
    pub fn from_raw(value: T) -> Self {
        Self::Raw(value)
    }

    pub fn from_serialized(value: SerializedFunctionResource<T>) -> Self {
        Self::Serialized(value)
    }

    pub async fn copy_to_host(self) -> T {
        match self {
            Self::Raw(v) => v,
            Self::Serialized(v) => v.copy_to_host().await,
        }
    }

    pub async fn move_to_host(self) -> T {
        match self {
            Self::Raw(v) => v,
            Self::Serialized(v) => v.move_to_host().await,
        }
    }
}
