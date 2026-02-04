use {
    std::{sync::{OnceLock, Mutex, Arc}, cell::RefCell},
    futures::future::BoxFuture,
    slotmap::{SlotMap, DefaultKey, Key, KeyData},
    lazy_static,
    crate::handler::FunctionResponse,
};

// TODO: implement drop for resources!
static FUNCTION_RESOURCES: OnceLock<Arc<Mutex<SlotMap<DefaultKey, Arc<FunctionResource>>>>> = OnceLock::new();

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
    FunctionResponseFuture(Mutex<BoxFuture<'static, FunctionResponse>>),
    FunctionResponse(FunctionResponse),
}

pub fn add_function_resource(resource: FunctionResource) -> FunctionResourceId {
    FunctionResourceId::new(function_resources().lock().unwrap().insert(Arc::new(resource)).data().as_ffi())
}

pub fn get_function_resource(resource: &FunctionResourceId) -> Arc<FunctionResource> {
    function_resources().lock().unwrap().get(resource.into()).unwrap().clone()
}

pub fn swap_function_resource(resource_id: &FunctionResourceId, new_resource: FunctionResource) -> Arc<FunctionResource> {
    std::mem::replace(function_resources().lock().unwrap().get_mut(resource_id.into()).unwrap(), Arc::new(new_resource))
}

fn function_resources() -> Arc<Mutex<SlotMap<DefaultKey, Arc<FunctionResource>>>> {
    FUNCTION_RESOURCES.get_or_init(|| Arc::new(Mutex::new(SlotMap::new()))).clone()
}
