use {
    std::{marker::PhantomData, rc::Rc},
    hyper::body::Bytes,
    crate::{
        function::abi::{capnp, abi_http_capnp},
        triggers::http::FunctionResponse,
        resources::resource::OwnedFunctionResourceId,
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
    let serialized_frame = message.init_root::<abi_http_capnp::http_request_body_frame::Builder>();
    let mut serialized_frame = serialized_frame.init_body();

    match frame {
        None => serialized_frame.set_stream_end(()),
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

    async fn move_to_host(self) -> T {
        let (instance, resource) = self.resource.consume();
        T::deserialize(&mut instance.move_serializable_resource_to_host(&resource).await.as_slice(), instance)
    }
}

impl SerializeResource for Vec<u8> {
    fn serialize(self) -> Vec<u8> {
        self
    }
}

trait DeserializeFunctionResource {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self;
}

impl DeserializeFunctionResource for FunctionResponse {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let response = message_reader.get_root::<abi_function_resources_capnp::function_response::Reader>().unwrap();
        Self(FunctionResponseInner::HttpResponse(FunctionHttpResponse {
            status: ::http::StatusCode::from_u16(response.get_status()).unwrap(),
            body: Cell::new(Some(SerializedFunctionResource::new(instance, FunctionResourceId::from(response.get_body_resource())))),
        }))
    }
}

impl DeserializeFunctionResource for Vec<u8> {
    fn deserialize(resource: &mut &[u8], _instance: Rc<FunctionInstance>) -> Self {
        resource.to_vec()
    }
}
