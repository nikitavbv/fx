use {
    std::{sync::{OnceLock, Mutex, Arc}, cell::{RefCell, LazyCell, OnceCell, Cell}, marker::PhantomData, borrow::BorrowMut},
    futures::future::LocalBoxFuture,
    slotmap::{SlotMap, DefaultKey, Key, KeyData},
    lazy_static,
    fx_types::{capnp, abi::FuturePollResult, abi_http_capnp},
    crate::{
        handler_fn::{FunctionResponse, FunctionResponseInner, FunctionHttpResponse},
        sys::{fx_resource_serialize, fx_resource_move_from_host, fx_resource_drop, fx_future_poll},
        api::http::{HttpBody, HttpBodyInner, serialize_http_body_full},
    },
};

thread_local! {
    static FUNCTION_RESOURCES: RefCell<SlotMap<DefaultKey, FunctionResource>> = RefCell::new(SlotMap::new());
}

pub struct ResourceId {
    id: u64,
}

impl ResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub(crate) fn as_ffi(&self) -> u64 {
        self.id
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

pub enum FunctionResource {
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

pub enum SerializableResource<T: SerializeResource> {
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
        let mut resource = message.init_root::<abi_http_capnp::function_response::Builder>();
        match self.0 {
            FunctionResponseInner::HttpResponse(http) => {
                resource.set_status(http.status.as_u16());

                let mut headers = resource.reborrow().init_headers(http.headers.len() as u32);
                for (index, (name, value)) in http.headers.iter().enumerate() {
                    let mut header = headers.reborrow().get(index as u32);
                    header.set_name(name.as_str());
                    header.set_value(value.to_str().unwrap());
                }

                resource.set_body_resource(http.body.as_u64());
            }
        }
        capnp::serialize::write_message_to_words(&message)
    }
}

/// Resource that origins from host side and is now owned by function.
/// moved lazily from host to function memory.
/// if dropped before being moved, cleans up resource on host side.
pub struct DeserializableHostResource<T: DeserializeHostResource>(LazyCell<T, Box<dyn FnOnce() -> T + Send>>);

impl<T: DeserializeHostResource> From<ResourceId> for DeserializableHostResource<T> {
    fn from(resource_id: ResourceId) -> Self {
        let resource_id = OwnedResourceId::new(resource_id);
        Self(LazyCell::new(Box::new(move || {
            let resource_id = resource_id.consume();
            let resource_length = unsafe { fx_resource_serialize(resource_id.id) } as usize;
            let data: Vec<u8> = vec![0u8; resource_length];
            unsafe { fx_resource_move_from_host(resource_id.id, data.as_ptr() as u64); }
            T::deserialize(&mut data.as_slice())
        })))
    }
}

/// Resource handle that is owned by function.
/// Cleans up host memory if dropped before being consmumed
pub struct OwnedResourceId(Cell<Option<ResourceId>>);

impl OwnedResourceId {
    pub fn new(resource_id: ResourceId) -> Self {
        Self(Cell::new(Some(resource_id)))
    }

    pub fn from_ffi(id: u64) -> Self {
        Self::new(ResourceId::new(id))
    }

    pub fn with<T, F: FnOnce(&ResourceId) -> T>(&self, mapper: F) -> T {
        let resource_id = self.0.replace(None).unwrap();
        let result = mapper(&resource_id);
        self.0.replace(Some(resource_id));
        result
    }

    pub fn consume(self) -> ResourceId {
        self.0.replace(None).unwrap()
    }
}

impl Drop for OwnedResourceId {
    fn drop(&mut self) {
        if let Some(resource) = self.0.replace(None) {
            unsafe { fx_resource_drop(resource.id) }
        }
    }
}

impl<T: DeserializeHostResource> DeserializableHostResource<T> {
    pub(crate) fn get_raw(&self) -> &T {
        &*self.0
    }

    pub(crate) fn get_raw_mut(&mut self) -> &mut T {
        &mut *self.0
    }
}

pub trait DeserializeHostResource {
    fn deserialize(data: &mut &[u8]) -> Self;
}

impl DeserializeHostResource for Vec<u8> {
    fn deserialize(data: &mut &[u8]) -> Self {
        data.to_vec()
    }
}

pub(crate) struct FutureHostResource<T: DeserializeHostResource> {
    resource_id: RefCell<Option<OwnedResourceId>>,
    _t: PhantomData<T>,
}

impl<T: DeserializeHostResource> FutureHostResource<T> {
    pub fn new(resource_id: OwnedResourceId) -> Self {
        Self {
            resource_id: RefCell::new(Some(resource_id)),
            _t: PhantomData,
        }
    }
}

impl<T: DeserializeHostResource> Future for FutureHostResource<T> {
    type Output = T;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let poll_result = self.resource_id.borrow().as_ref().unwrap().with(|resource_id| {
            unsafe { fx_future_poll(resource_id.id) }
        });
        let poll_result = FuturePollResult::try_from(poll_result).unwrap();
        match poll_result {
            FuturePollResult::Pending => std::task::Poll::Pending,
            FuturePollResult::Ready => std::task::Poll::Ready({
                let resource_id = self.resource_id.replace(None).unwrap().consume();
                let resource_length = unsafe { fx_resource_serialize(resource_id.id) } as usize;
                let data: Vec<u8> = vec![0u8; resource_length];
                unsafe { fx_resource_move_from_host(resource_id.id, data.as_ptr() as u64); }
                T::deserialize(&mut data.as_slice())
            }),
        }
    }
}

pub(crate) struct HostUnitFuture {
    resource_id: OwnedResourceId,
}

impl HostUnitFuture {
    pub fn new(resource_id: OwnedResourceId) -> Self {
        Self {
            resource_id,
        }
    }
}

impl Future for HostUnitFuture {
    type Output = ();

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let poll_result = self.resource_id.with(|resource_id| {
            unsafe { fx_future_poll(resource_id.id) }
        });
        match FuturePollResult::try_from(poll_result).unwrap() {
            FuturePollResult::Pending => std::task::Poll::Pending,
            FuturePollResult::Ready => std::task::Poll::Ready(()),
        }
    }
}

pub fn add_function_resource(resource: FunctionResource) -> FunctionResourceId {
    FUNCTION_RESOURCES.with_borrow_mut(|v| FunctionResourceId::new(v.insert(resource).data().as_ffi()))
}

pub fn map_function_resource_ref<T, F: FnOnce(&FunctionResource) -> T>(resource_id: &FunctionResourceId, mapper: F) -> T {
    let resource = FUNCTION_RESOURCES.with_borrow_mut(|v| v.detach(resource_id.into()).unwrap());

    let result = mapper(&resource);

    FUNCTION_RESOURCES.with_borrow_mut(|v| v.reattach(resource_id.into(), resource));

    result
}

pub fn map_function_resource_ref_mut<T, F: FnOnce(&mut FunctionResource) -> T>(resource_id: &FunctionResourceId, mapper: F) -> T {
    let mut resource = FUNCTION_RESOURCES.with_borrow_mut(|v| v.detach(resource_id.into()).unwrap());

    let result = mapper(&mut resource);

    FUNCTION_RESOURCES.with_borrow_mut(|v| v.reattach(resource_id.into(), resource));

    result
}

pub fn replace_function_resource(resource_id: &FunctionResourceId, new_resource: FunctionResource) {
    FUNCTION_RESOURCES.with_borrow_mut(|resources| {
        *resources.get_mut(resource_id.into()).unwrap() = new_resource;
    })
}

pub fn replace_function_resource_with<F: FnOnce(FunctionResource) -> FunctionResource>(resource_id: FunctionResourceId, mapper: F) {
    FUNCTION_RESOURCES.with_borrow_mut(move |resources| {
        let resource = resources.detach((&resource_id).into()).unwrap();
        let resource = mapper(resource);
        resources.reattach((&resource_id).into(), resource);
    })
}

pub fn replace_function_resource_with_effect<T, F: FnOnce(FunctionResource) -> (FunctionResource, T)>(resource_id: FunctionResourceId, mapper: F) -> T {
    FUNCTION_RESOURCES.with_borrow_mut(move |resources| {
        let resource = resources.detach((&resource_id).into()).unwrap();
        let (resource, effect) = mapper(resource);
        resources.reattach((&resource_id).into(), resource);
        effect
    })
}

/// returns length of serialized resource
pub fn serialize_function_resource(resource_id: &FunctionResourceId) -> u64 {
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
                HttpBodyInner::Stream(_) => {
                    let mut message = capnp::message::Builder::new_default();
                    let serialized_body = message.init_root::<abi_http_capnp::http_body::Builder>();
                    let mut serialized_body = serialized_body.init_body();

                    serialized_body.set_stream(());

                    let serialized_body = capnp::serialize::write_message_to_words(&message);
                    let serialized_len = serialized_body.len();

                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(serialized_body))), serialized_len)
                },
                HttpBodyInner::PartiallyReadStream { .. } => panic!("resource of this type cannot be serialized"),
                HttpBodyInner::HostResource(resource_id) => {
                    let resource_id = resource_id.consume();

                    let mut message = capnp::message::Builder::new_default();
                    let http_body = message.init_root::<abi_http_capnp::http_body::Builder>();
                    let mut http_body = http_body.init_body();
                    http_body.set_host_resource(resource_id.as_ffi());

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

pub fn drop_function_resource(resource_id: &FunctionResourceId) {
    FUNCTION_RESOURCES.with_borrow_mut(|v| v.remove(resource_id.into()));
}
