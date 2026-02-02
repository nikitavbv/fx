use {
    std::{sync::{OnceLock, Mutex, Arc}, cell::RefCell},
    futures::future::BoxFuture,
    slotmap::{SlotMap, DefaultKey, Key},
    lazy_static,
    crate::handler::FunctionResponse,
};

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

pub enum FunctionResource {
    FunctionResponseFuture(Mutex<BoxFuture<'static, FunctionResponse>>),
}

pub fn add_function_resource(resource: FunctionResource) -> FunctionResourceId {
    FunctionResourceId::new(function_resources().lock().unwrap().insert(Arc::new(resource)).data().as_ffi())
}

fn function_resources() -> Arc<Mutex<SlotMap<DefaultKey, Arc<FunctionResource>>>> {
    FUNCTION_RESOURCES.get_or_init(|| Arc::new(Mutex::new(SlotMap::new()))).clone()
}
