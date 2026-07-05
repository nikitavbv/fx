use {
    std::cell::RefCell,
    futures::future::LocalBoxFuture,
    slotmap::{SlotMap, DefaultKey, Key, KeyData},
    thiserror::Error,
    fx_types::{capnp, abi::{UnitFuturePollResult}, abi_http_capnp},
    crate::{
        handler_fn::{FunctionResponse, FunctionResponseInner},
        sys::{
            fx_bytes_len,
            fx_bytes_move,
            fx_unit_future_poll,
        },
        api::http::{HttpBody, HttpBodyInner, serialize_http_body_full},
    },
};

thread_local! {
    static FUNCTION_RESOURCES: RefCell<SlotMap<DefaultKey, FunctionResource>> = RefCell::new(SlotMap::new());
}

pub struct FetchRequestHeaderResourceId {
    id: u64,
    is_consumed: bool,
}

impl FetchRequestHeaderResourceId {
    pub fn consume_for_ffi(mut self) -> u64 {
        self.is_consumed = true;
        self.id
    }
}

impl From<u64> for FetchRequestHeaderResourceId {
    fn from(id: u64) -> Self {
        Self { id, is_consumed: false }
    }
}

pub struct BytesResource {
    resource_id: u64,
    is_consumed: bool,
}

impl BytesResource {
    pub fn into_vec(mut self) -> Vec<u8> {
        let length = unsafe { fx_bytes_len(self.resource_id) } as usize;
        let data: Vec<u8> = vec![0u8; length];
        unsafe { fx_bytes_move(self.resource_id, data.as_ptr() as u64); }
        self.is_consumed = true;
        data
    }
}

impl From<u64> for BytesResource {
    fn from(resource_id: u64) -> Self {
        Self { resource_id, is_consumed: true }
    }
}

pub struct FunctionResourceId {
    id: u64,
}

impl FunctionResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl Into<DefaultKey> for &FunctionResourceId {
    fn into(self) -> DefaultKey {
        DefaultKey::from(KeyData::from_ffi(self.as_u64()))
    }
}

pub(crate) enum FunctionResource {
    FunctionResponseFuture(LocalBoxFuture<'static, FunctionResponse>),
    FunctionResponse(SerializableResource<FunctionResponse>),
    HttpBody(HttpBody),
    BackgroundTask(LocalBoxFuture<'static, ()>),
}

impl From<FunctionResponse> for FunctionResource {
    fn from(value: FunctionResponse) -> Self {
        Self::FunctionResponse(SerializableResource::Raw(value))
    }
}

pub(crate) enum SerializableResource<T: SerializeResource> {
    Raw(T),
    Serialized(Vec<u8>),
}

impl<T: SerializeResource> SerializableResource<T> {
    fn map_to_serialized(self) -> Self {
        match self {
            Self::Serialized(v) => Self::Serialized(v),
            Self::Raw(v) => Self::Serialized(v.serialize()),
        }
    }

    fn serialized_size(&self) -> usize {
        match self {
            Self::Raw(_) => panic!("cannot compute serialized size for resource that is not serialized yet"),
            Self::Serialized(v) => v.len(),
        }
    }
}

pub(crate) trait SerializeResource {
    fn serialize(self) -> Vec<u8>;
}

impl SerializeResource for FunctionResponse {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut resource = message.init_root::<abi_http_capnp::http_response::Builder>();
        match self.0 {
            FunctionResponseInner::HttpResponse(http) => {
                resource.set_status(http.status.as_u16());

                let mut headers = resource.reborrow().init_headers(http.headers.len() as u32);
                for (index, (name, value)) in http.headers.iter().enumerate() {
                    let mut header = headers.reborrow().get(index as u32);
                    header.set_name(name.as_str());
                    header.set_value(value.to_str().unwrap());
                }

                resource.set_body_resource_id(http.body.as_u64());
            }
        }
        capnp::serialize::write_message_to_words(&message)
    }
}

mod host_unit_future {
    use super::*;

    pub(crate) struct HostUnitFuture(u64);

    impl HostUnitFuture {
        pub fn new(resource_id: u64) -> Self {
            Self(resource_id)
        }
    }

    impl Future for HostUnitFuture {
        type Output = Result<(), PollError>;

        fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
            let mut result = std::mem::MaybeUninit::<UnitFuturePollResult>::zeroed();
            assert!(unsafe { fx_unit_future_poll(self.0, result.as_mut_ptr() as u64) } == 0);

            let result = unsafe { result.assume_init() };

            match result.tag {
                1 => std::task::Poll::Pending,
                0 => std::task::Poll::Ready(Ok(())),
                _other => std::task::Poll::Ready(Err(PollError::InternalSdkError)),
            }
        }
    }

    #[derive(Debug, Error)]
    pub(crate) enum PollError {
        #[error("internal sdk error")]
        InternalSdkError,
    }
}
pub(crate) use host_unit_future::HostUnitFuture;

pub(crate) fn add_function_resource(resource: FunctionResource) -> FunctionResourceId {
    FUNCTION_RESOURCES.with_borrow_mut(|v| FunctionResourceId::new(v.insert(resource).data().as_ffi()))
}

pub(crate) fn map_function_resource_ref<T, F: FnOnce(&FunctionResource) -> T>(resource_id: &FunctionResourceId, mapper: F) -> T {
    let resource = FUNCTION_RESOURCES.with_borrow_mut(|v| v.detach(resource_id.into()).unwrap());

    let result = mapper(&resource);

    FUNCTION_RESOURCES.with_borrow_mut(|v| v.reattach(resource_id.into(), resource));

    result
}

pub(crate) fn map_function_resource_ref_mut<T, F: FnOnce(&mut FunctionResource) -> T>(resource_id: &FunctionResourceId, mapper: F) -> T {
    let mut resource = FUNCTION_RESOURCES.with_borrow_mut(|v| v.detach(resource_id.into()).unwrap());

    let result = mapper(&mut resource);

    FUNCTION_RESOURCES.with_borrow_mut(|v| v.reattach(resource_id.into(), resource));

    result
}

pub(crate) fn replace_function_resource(resource_id: &FunctionResourceId, new_resource: FunctionResource) {
    FUNCTION_RESOURCES.with_borrow_mut(|resources| {
        *resources.get_mut(resource_id.into()).unwrap() = new_resource;
    })
}

pub(crate) fn replace_function_resource_with<F: FnOnce(FunctionResource) -> FunctionResource>(resource_id: FunctionResourceId, mapper: F) {
    FUNCTION_RESOURCES.with_borrow_mut(move |resources| {
        let resource = resources.detach((&resource_id).into()).unwrap();
        let resource = mapper(resource);
        resources.reattach((&resource_id).into(), resource);
    })
}

pub(crate) fn replace_function_resource_with_effect<T, F: FnOnce(FunctionResource) -> (FunctionResource, T)>(resource_id: FunctionResourceId, mapper: F) -> T {
    FUNCTION_RESOURCES.with_borrow_mut(move |resources| {
        let resource = resources.detach((&resource_id).into()).unwrap();
        let (resource, effect) = mapper(resource);
        resources.reattach((&resource_id).into(), resource);
        effect
    })
}

/// returns length of serialized resource
pub(crate) fn serialize_function_resource(resource_id: &FunctionResourceId) -> u64 {
    FUNCTION_RESOURCES.with_borrow_mut(|resources| {
        let resource = resources.detach(resource_id.into()).unwrap();
        let (resource, serialized_size) = match resource {
            FunctionResource::FunctionResponseFuture(_) |
            FunctionResource::BackgroundTask(_) => panic!("this type of resource cannot be serialized"),
            FunctionResource::FunctionResponse(v) => {
                let serialized = v.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (FunctionResource::FunctionResponse(serialized), serialized_size)
            },
            FunctionResource::HttpBody(v) => match v.0 {
                HttpBodyInner::Empty => {
                    let mut message = capnp::message::Builder::new_default();
                    let serialized_body = message.init_root::<abi_http_capnp::http_body::Builder>();
                    let mut serialized_body = serialized_body.init_body();

                    serialized_body.set_empty(());

                    let serialized_frame = capnp::serialize::write_message_to_words(&message);
                    let serialized_len = serialized_frame.len();

                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(serialized_frame))), serialized_len)
                },
                HttpBodyInner::Bytes(v) => {
                    let serialized = serialize_http_body_full(v);
                    let serialized_size = serialized.len();
                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(serialized))), serialized_size)
                },
                HttpBodyInner::Stream { stream, frame_serialized } => {
                    let mut message = capnp::message::Builder::new_default();
                    let serialized_body = message.init_root::<abi_http_capnp::http_body::Builder>();
                    let mut serialized_body = serialized_body.init_body();

                    let stream_resource_id = resources.insert(FunctionResource::HttpBody(HttpBody(HttpBodyInner::Stream { stream, frame_serialized })));
                    let stream_resource_id = FunctionResourceId::new(stream_resource_id.data().as_ffi());

                    serialized_body.set_function_stream(stream_resource_id.as_u64());

                    let serialized_body = capnp::serialize::write_message_to_words(&message);
                    let serialized_len = serialized_body.len();

                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(serialized_body))), serialized_len)
                },
                HttpBodyInner::HostResource { resource_id, frame_resource_id } => {
                    let mut message = capnp::message::Builder::new_default();
                    let http_body = message.init_root::<abi_http_capnp::http_body::Builder>();
                    let mut http_body = http_body.init_body();
                    http_body.set_host_resource(resource_id);

                    let serialized_frame = capnp::serialize::write_message_to_words(&message);
                    let serialized_len = serialized_frame.len();

                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(serialized_frame))), serialized_len)
                },
                HttpBodyInner::Serialized(v) => {
                    let serialized_len = v.len();
                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(v))), serialized_len)
                },
            },
        };
        resources.reattach(resource_id.into(), resource);
        serialized_size
    }) as u64
}

pub(crate) fn drop_function_resource(resource_id: &FunctionResourceId) {
    FUNCTION_RESOURCES.with_borrow_mut(|v| v.remove(resource_id.into()));
}
