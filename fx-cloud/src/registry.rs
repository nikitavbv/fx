use {
    std::{sync::Arc, collections::HashMap},
    crate::storage::{KVStorage, BoxedStorage},
};

// TODO: implement registry
pub struct KVRegistry {

}

impl KVRegistry {
    pub fn new() -> Self {
        Self {}
    }

    pub fn register<T: KVStorage>(&self, id: String, storage: T) {

    }
}
