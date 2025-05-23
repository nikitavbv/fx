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
    pub sql: Vec<SqlDefinition>,
}

impl Default for FunctionDefinition {
    fn default() -> Self {
        Self {
            sql: Vec::new(),
        }
    }
}

pub struct SqlDefinition {
    pub id: String,
    pub path: String,
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
    let config: FunctionConfig = serde_yml::from_slice(&config).unwrap();
    FunctionDefinition {
        sql: config.sql.unwrap_or(Vec::new())
            .into_iter()
            .map(|v| SqlDefinition {
                id: v.id,
                path: v.path,
            })
            .collect(),
    }
}

#[derive(Deserialize)]
struct FunctionConfig {
    sql: Option<Vec<SqlConfig>>,
}

#[derive(Deserialize)]
struct SqlConfig {
    id: String,
    path: String,
}
