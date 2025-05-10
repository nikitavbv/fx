use {
    std::{collections::HashMap, fs},
    serde_yml::Value,
    serde::Deserialize,
    crate::{
        storage::{BoxedStorage, SqliteStorage, WithKey},
        sql::SqlDatabase,
        error::FxCloudError,
    },
};

#[derive(Deserialize, Debug)]
pub struct Config {
    pub kv: Vec<ConfigKv>,
    pub sql: Vec<ConfigSql>,
}

#[derive(Deserialize, Debug)]
pub struct ConfigKv {
    pub id: String,
    pub driver: String,
    pub params: HashMap<String, Value>,
    pub keys: Option<Vec<ConfigKvKey>>,
}

#[derive(Deserialize, Debug)]
pub struct SqliteParams {
    in_memory: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct ConfigKvKey {
    key: String,
    file: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ConfigSql {
    pub id: String,
}

impl Config {
    pub fn load(config_str: &str) -> Self {
        serde_yml::from_str(config_str).unwrap()
    }
}

pub fn kv_from_config(config: &ConfigKv) -> Result<BoxedStorage, FxCloudError> {
    let params: Value = serde_yml::to_value(&config.params).unwrap();

    let mut storage = match config.driver.as_str() {
        "sqlite" => BoxedStorage::new({
            let params: SqliteParams = serde_yml::from_value(params).unwrap();
            if params.in_memory.unwrap_or(false) {
                SqliteStorage::in_memory().unwrap()
            } else {
                panic!("memory-based kv is not implemented yet")
            }
        }),
        other => panic!("unknown kv driver: {other:?}"),
    };

    for kv in config.keys.as_ref().unwrap_or(&Vec::new()) {
        let key = kv.key.clone();
        let value = match fs::read(kv.file.as_ref().unwrap()) {
            Ok(v) => v,
            Err(err) => panic!("failed to read value from file: {}, reason: {err:?}", kv.file.as_ref().unwrap()),
        };
        storage = storage.with_key(key.as_bytes(), &value)?;
    }

    Ok(storage)
}

pub fn sql_from_config(_config: &ConfigSql) -> SqlDatabase {
    SqlDatabase::in_memory()
}
