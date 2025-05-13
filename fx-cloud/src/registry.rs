use {
    std::{sync::{Arc, RwLock}, collections::HashMap},
    crate::{
        storage::BoxedStorage,
        sql::SqlDatabase,
        error::FxCloudError,
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

    pub fn register(&self, id: String, storage: BoxedStorage) -> Result<(), FxCloudError> {
        self.registry.write()
            .map_err(|err| FxCloudError::ConfigurationError {
                reason: format!("failed to acquire kv registry lock: {err:?}"),
            })?
            .insert(id, storage);
        Ok(())
    }

    pub fn get(&self, id: String) -> Result<BoxedStorage, FxCloudError> {
        Ok(self.registry.read()
            .map_err(|err| FxCloudError::ConfigurationError {
                reason: format!("failed to acquire kv registry lock: {err:?}"),
            })?
            .get(&id).unwrap().clone())
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
