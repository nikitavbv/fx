use {
    std::{sync::{Arc, RwLock}, collections::HashMap},
    crate::{
        storage::BoxedStorage,
        sql::SqlDatabase,
    },
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

pub struct SqlRegistry {
    registry: Arc<RwLock<HashMap<String, SqlDatabase>>>,
}

impl SqlRegistry {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(&self, id: String, sql: SqlDatabase) {
        self.registry.write().unwrap().insert(id, sql);
    }

    pub fn get(&self, id: String) -> SqlDatabase {
        self.registry.read().unwrap().get(&id).unwrap().clone()
    }
}
