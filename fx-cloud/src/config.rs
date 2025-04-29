use {
    std::collections::HashMap,
    serde_yml::Value,
    serde::Deserialize,
};

#[derive(Deserialize, Debug)]
pub struct Config {
    kv: Vec<ConfigKv>,
}

#[derive(Deserialize, Debug)]
pub struct ConfigKv {
    id: String,
    driver: String,
    params: HashMap<String, Value>,
    keys: Option<Vec<ConfigKvKey>>,
}

#[derive(Deserialize, Debug)]
pub struct ConfigKvKey {
    key: String,
    file: Option<String>,
}

impl Config {
    pub fn load(config_str: &str) -> Self {
        serde_yml::from_str(config_str).unwrap()
    }
}
