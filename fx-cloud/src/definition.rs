use {
    std::{collections::HashMap, fs},
    serde_yml::Value,
    serde::Deserialize,
    thiserror::Error,
    crate::{
        kv::{BoxedStorage, SqliteStorage, WithKey, KVStorage},
        sql::{SqlDatabase, SqlError},
        error::FxCloudError,
        cloud::ServiceId,
    },
};

#[derive(Clone)]
pub struct FunctionDefinition {
    pub kv: Vec<KvDefinition>,
    pub sql: Vec<SqlDefinition>,
}

impl FunctionDefinition {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_kv(mut self, kv: KvDefinition) -> Self {
        self.kv.push(kv);
        self
    }

    pub fn with_sql(mut self, sql: SqlDefinition) -> Self {
        self.sql.push(sql);
        self
    }
}

impl Default for FunctionDefinition {
    fn default() -> Self {
        Self {
            kv: Vec::new(),
            sql: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct KvDefinition {
    pub id: String,
    pub path: String,
}

impl KvDefinition {
    pub fn new(id: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
        }
    }
}

#[derive(Clone)]
pub struct SqlDefinition {
    pub id: String,
    pub storage: SqlStorageDefinition,
}

#[derive(Clone)]
pub enum SqlStorageDefinition {
    InMemory,
    Path(String),
}

impl SqlDefinition {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            storage: SqlStorageDefinition::InMemory,
        }
    }
}

pub struct DefinitionProvider {
    storage: BoxedStorage,
    definitions: HashMap<ServiceId, FunctionDefinition>,
}

impl DefinitionProvider {
    pub fn new(storage: BoxedStorage) -> Self {
        Self {
            storage,
            definitions: HashMap::new(),
        }
    }

    pub fn with_definition(mut self, service_id: ServiceId, definition: FunctionDefinition) -> Self {
        self.definitions.insert(service_id, definition);
        self
    }

    pub fn definition_for_function(&self, id: &ServiceId) -> Result<FunctionDefinition, DefinitionError> {
        if let Some(definition) = self.definitions.get(id) {
            return Ok(definition.clone());
        }

        Ok(self.storage.get(<&ServiceId as Into<String>>::into(id).as_bytes())
            .unwrap()
            .map(|v| definition_from_config(v))
            .transpose()?
            .unwrap_or(FunctionDefinition::default()))
    }
}

fn definition_from_config(config: Vec<u8>) -> Result<FunctionDefinition, DefinitionError> {
    let config: FunctionConfig = serde_yml::from_slice(&config)
        .map_err(|err| DefinitionError::ParseError { reason: format!("failed to load yaml file: {err:?}") })?;
    Ok(FunctionDefinition {
        kv: config.kv.unwrap_or(Vec::new())
            .into_iter()
            .map(|v| KvDefinition {
                id: v.id,
                path: v.path,
            })
            .collect(),
        sql: config.sql.unwrap_or(Vec::new())
            .into_iter()
            .map(|v| SqlDefinition {
                id: v.id,
                storage: SqlStorageDefinition::Path(v.path),
            })
            .collect(),
    })
}

#[derive(Deserialize)]
struct FunctionConfig {
    kv: Option<Vec<KvConfig>>,
    sql: Option<Vec<SqlConfig>>,
}

#[derive(Deserialize)]
struct KvConfig {
    id: String,
    path: String,
}

#[derive(Deserialize)]
struct SqlConfig {
    id: String,
    path: String,
}

#[derive(Deserialize)]
pub struct CronConfig {
    pub state_path: String,
    pub tasks: Vec<CronTaskConfig>,
}

#[derive(Deserialize)]
pub struct CronTaskConfig {
    pub id: String,
    pub schedule: String,
    pub function: String,
    pub rpc_method_name: String,
}

#[derive(Error, Debug)]
pub enum DefinitionError {
    #[error("failed to parse definition: {reason}")]
    ParseError { reason: String },
}

pub fn load_cron_task_from_config(config: Vec<u8>) -> CronConfig {
    serde_yml::from_slice(&config).unwrap()
}

#[derive(Deserialize)]
pub struct RabbitMqConsumerConfig {
    pub consumers: Vec<RabbitMqConsumerTaskConfig>,
}

#[derive(Deserialize)]
pub struct RabbitMqConsumerTaskConfig {
    pub id: String,
    pub queue: String,
    pub function: String,
    pub rpc_method_name: String,
}

pub fn load_rabbitmq_consumer_task_from_config(config: Vec<u8>) -> RabbitMqConsumerConfig {
    serde_yml::from_slice(&config).unwrap()
}
