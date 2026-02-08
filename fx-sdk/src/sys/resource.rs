use {
    std::{sync::{OnceLock, Mutex, Arc}, cell::{RefCell, LazyCell, OnceCell}, marker::PhantomData, borrow::BorrowMut},
    futures::future::LocalBoxFuture,
    slotmap::{SlotMap, DefaultKey, Key, KeyData},
    lazy_static,
    fx_types::{capnp, abi_function_resources_capnp},
    crate::{
        handler::{FunctionResponse, FunctionResponseInner, FunctionHttpResponse},
        sys::{fx_resource_serialize, fx_resource_move_from_host},
    },
};

// TODO: implement drop for resources!
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
    fn serialize_inplace(&mut self) -> usize {
        let prev = std::mem::replace(self, SerializableResource::Serialized(Vec::new()));
        let mut len = 0;
        *self = match prev {
            Self::Serialized(v) => {
                len = v.len();
                Self::Serialized(v)
            },
            Self::Raw(v) => {
                let serialized = v.serialize();
                len = serialized.len();
                Self::Serialized(serialized)
            },
        };
        len
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
            }
        }
        capnp::serialize::write_message_to_words(&message)
    }
}

pub struct DeserializableHostResource<T: DeserializeHostResource>(LazyCell<T, Box<dyn FnOnce() -> T>>);

impl<T: DeserializeHostResource> From<ResourceId> for DeserializableHostResource<T> {
    fn from(resource_id: ResourceId) -> Self {
        Self(LazyCell::new(Box::new(move || {
            let resource_length = unsafe { fx_resource_serialize(resource_id.id) } as usize;
            let data: Vec<u8> = vec![0u8; resource_length];
            unsafe { fx_resource_move_from_host(resource_id.id, data.as_ptr() as u64); }
            T::deserialize(&mut data.as_slice())
        })))
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

pub fn serialize_function_resource(resource_id: &FunctionResourceId) -> u64 {
    FUNCTION_RESOURCES.with_borrow_mut(|resources| {
        let resource = resources.get_mut(resource_id.into()).unwrap();
        (match &mut *resource {
            FunctionResource::FunctionResponseFuture(_) => panic!("this type of resource cannot be serialized"),
            FunctionResource::FunctionResponse(v) => v.serialize_inplace(),
        }) as u64
    })
}

pub fn drop_function_resource(resource_id: &FunctionResourceId) {
    FUNCTION_RESOURCES.with_borrow_mut(|v| v.remove(resource_id.into()));
}
