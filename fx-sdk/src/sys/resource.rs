use {
    std::{sync::{OnceLock, Mutex, Arc}, cell::{RefCell, LazyCell, OnceCell, Cell}, marker::PhantomData, borrow::BorrowMut},
    futures::future::LocalBoxFuture,
    slotmap::{SlotMap, DefaultKey, Key, KeyData},
    lazy_static,
    fx_types::{capnp, abi::FuturePollResult, abi_function_resources_capnp},
    crate::{
        handler::{FunctionResponse, FunctionResponseInner, FunctionHttpResponse},
        sys::{fx_resource_serialize, fx_resource_move_from_host, fx_resource_drop, fx_future_poll},
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
    FunctionResponseBody(Vec<u8>),
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

trait SerializeResource {
    fn serialize(self) -> Vec<u8>;
}

impl SerializeResource for FunctionResponse {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut resource = message.init_root::<abi_function_resources_capnp::function_response::Builder>();
        match self.0 {
            FunctionResponseInner::HttpResponse(http) => {
                resource.set_status(http.status.as_u16());
                resource.set_body_resource(http.body.as_u64());
            }
        }
        capnp::serialize::write_message_to_words(&message)
    }
}

/// Resource that origins from host side and is now owned by function.
/// moved lazily from host to function memory.
/// if dropped before being moved, cleans up resource on host side.
pub struct DeserializableHostResource<T: DeserializeHostResource>(LazyCell<T, Box<dyn FnOnce() -> T>>);

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
}

pub trait DeserializeHostResource {
    fn deserialize(data: &mut &[u8]) -> Self;
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

/// returns length of serialized resource
pub fn serialize_function_resource(resource_id: &FunctionResourceId) -> u64 {
    FUNCTION_RESOURCES.with_borrow_mut(|resources| {
        let resource = resources.detach(resource_id.into()).unwrap();
        let (resource, serialized_size) = match resource {
            FunctionResource::FunctionResponseFuture(_) => panic!("this type of resource cannot be serialized"),
            FunctionResource::FunctionResponse(v) => {
                let serialized = v.map_to_serialized();
                let serialized_size = serialized.serialized_size();
                (FunctionResource::FunctionResponse(serialized), serialized_size)
            },
            FunctionResource::FunctionResponseBody(v) => {
                let serialized_size = v.len();
                (FunctionResource::FunctionResponseBody(v), serialized_size)
            },
        };
        resources.reattach(resource_id.into(), resource);
        serialized_size
    }) as u64
}

pub fn drop_function_resource(resource_id: &FunctionResourceId) {
    FUNCTION_RESOURCES.with_borrow_mut(|v| v.remove(resource_id.into()));
}
