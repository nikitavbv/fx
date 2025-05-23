use {
    std::{collections::HashMap, fs},
    serde_yml::Value,
    serde::Deserialize,
    crate::{
        storage::{BoxedStorage, SqliteStorage, WithKey, KVStorage},
        sql::{SqlDatabase, SqlError},
        error::FxCloudError,
        cloud::ServiceId,
    },
};

pub struct FunctionDefinition {
}

impl Default for FunctionDefinition {
    fn default() -> Self {
        Self {}
    }
}

pub struct DefinitionProvider {
    storage: BoxedStorage,
}

impl DefinitionProvider {
    pub fn new(storage: BoxedStorage) -> Self {
        Self {
            storage,
        }
    }

    pub fn definition_for_function(&self, id: &ServiceId) -> FunctionDefinition {
        self.storage.get(<&ServiceId as Into<String>>::into(id).as_bytes())
            .unwrap()
            .map(|v| definition_from_config(v))
            .unwrap_or(FunctionDefinition::default())
    }
}

fn definition_from_config(config: Vec<u8>) -> FunctionDefinition {
    FunctionDefinition {}
}
