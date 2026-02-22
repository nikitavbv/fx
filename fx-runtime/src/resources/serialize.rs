pub(crate) trait SerializeResource {
    fn serialize(self) -> Vec<u8>;
}

fn serialize_request_body_full(body: Vec<u8>) -> Vec<u8> {
    todo!("serialize request body full")
}

fn serialize_partially_read_stream(frame: Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>) -> Vec<u8> {
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
    fn map_to_serialized(self) -> Self {
        match self {
            Self::Raw(t) => Self::Serialized(t.serialize()),
            Self::Serialized(v) => Self::Serialized(v),
        }
    }

    fn serialized_size(&self) -> usize {
        match self {
            Self::Raw(_) => panic!("cannot compute serialized size for resource that is not serialized yet"),
            Self::Serialized(v) => v.len(),
        }
    }

    fn into_serialized(self) -> Vec<u8> {
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
