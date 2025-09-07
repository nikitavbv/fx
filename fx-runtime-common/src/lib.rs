use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
};

pub use crate::events::FunctionInvokeEvent;

pub mod events;

#[derive(Serialize, Deserialize)]
pub struct Function {
    pub id: String,
}

// logs
#[derive(Serialize, Deserialize, Debug)]
pub struct LogMessageEvent {
    pub source: LogSource,
    pub fields: HashMap<String, EventFieldValue>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum EventFieldValue {
    Text(String),
    Object(HashMap<String, Box<EventFieldValue>>),
}

impl LogMessageEvent {
    pub fn new(source: LogSource, fields: HashMap<String, String>) -> Self {
        Self {
            source,
            fields: fields.into_iter()
                .map(|(k, v)| (k, EventFieldValue::Text(v)))
                .collect(),
        }
    }

    pub fn source(&self) -> &LogSource {
        &self.source
    }

    pub fn fields(&self) -> &HashMap<String, EventFieldValue> {
        &self.fields
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum LogSource {
    Function {
        id: String,
    },
    FxRuntime,
}
