use {
    std::{sync::{Arc, RwLock}, collections::HashMap},
    crate::storage::{KVStorage, BoxedStorage},
};

pub struct KVRegistry {
    registry: Arc<RwLock<HashMap<String, BoxedStorage>>>,
}

impl KVRegistry {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(&self, id: String, storage: BoxedStorage) {
        self.registry.write().unwrap().insert(id, storage);
    }

    pub fn get(&self, id: String) -> BoxedStorage {
        self.registry.read().unwrap().get(&id).unwrap().clone()
    }
}
