use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
};

pub use crate::events::FunctionInvokeEvent;

pub mod events;
pub mod utils;

#[derive(Serialize, Deserialize)]
pub struct Function {
    pub id: String,
}

// logs
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogMessageEvent {
    pub source: LogSource,
    pub fields: HashMap<String, EventFieldValue>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum EventFieldValue {
    Text(String),
    U64(u64),
    I64(i64),
    F64(f64),
    Object(HashMap<String, Box<EventFieldValue>>),
}

impl LogMessageEvent {
    pub fn new(source: LogSource, fields: HashMap<String, EventFieldValue>) -> Self {
        Self {
            source,
            fields,
        }
    }

    pub fn source(&self) -> &LogSource {
        &self.source
    }

    pub fn fields(&self) -> &HashMap<String, EventFieldValue> {
        &self.fields
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LogSource {
    Function {
        id: String,
    },
    FxRuntime,
}
