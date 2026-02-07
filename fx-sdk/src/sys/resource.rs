use {
    std::{sync::{OnceLock, Mutex, Arc}, cell::LazyCell, marker::PhantomData},
    futures::future::BoxFuture,
    slotmap::{SlotMap, DefaultKey, Key, KeyData},
    lazy_static,
    fx_types::{capnp, abi_function_resources_capnp},
    crate::handler::{FunctionResponse, FunctionResponseInner, FunctionHttpResponse},
};

// TODO: implement drop for resources!
static FUNCTION_RESOURCES: OnceLock<Mutex<SlotMap<DefaultKey, FunctionResource>>> = OnceLock::new();

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
    FunctionResponseFuture(BoxFuture<'static, FunctionResponse>),
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

pub struct DeserializableHostResource<T: DeserializeHostResource>(LazyCell<T>);

impl<T: DeserializeHostResource> From<ResourceId> for DeserializableHostResource<T> {
    fn from(value: ResourceId) -> Self {
        Self(LazyCell::new(move || {
            let data: Vec<u8> = unimplemented!("fetch data by resource id!");
            T::deserialize(&mut data.as_slice())
        }))
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
    FunctionResourceId::new(function_resources().lock().unwrap().insert(resource).data().as_ffi())
}

pub fn map_function_resource_ref<T, F: FnOnce(&FunctionResource) -> T>(resource: &FunctionResourceId, mapper: F) -> T {
    mapper(function_resources().lock().unwrap().get(resource.into()).unwrap())
}

pub fn map_function_resource_ref_mut<T, F: FnOnce(&mut FunctionResource) -> T>(resource: &FunctionResourceId, mapper: F) -> T {
    mapper(function_resources().lock().unwrap().get_mut(resource.into()).unwrap())
}

pub fn replace_function_resource(resource_id: &FunctionResourceId, new_resource: FunctionResource) {
    *function_resources().lock().unwrap().get_mut(resource_id.into()).unwrap() = new_resource;
}

pub fn serialize_function_resource(resource_id: &FunctionResourceId) -> u64 {
    let resources = function_resources();
    let mut resources = resources.lock().unwrap();
    let resource = resources.get_mut(resource_id.into()).unwrap();
    (match &mut *resource {
        FunctionResource::FunctionResponseFuture(_) => panic!("this type of resource cannot be serialized"),
        FunctionResource::FunctionResponse(v) => v.serialize_inplace(),
    }) as u64
}

pub fn drop_function_resource(resource_id: &FunctionResourceId) {
    function_resources().lock().unwrap().remove(resource_id.into());
}

fn function_resources() -> &'static Mutex<SlotMap<DefaultKey, FunctionResource>> {
    FUNCTION_RESOURCES.get_or_init(|| Mutex::new(SlotMap::new()))
}
