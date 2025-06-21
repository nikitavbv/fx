use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
};

#[derive(Serialize, Deserialize)]
pub struct Function {
    pub id: String,
}

// events
#[derive(Serialize, Deserialize)]
pub struct FunctionInvokeEvent {
    pub function_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LogMessageEvent {
    source: LogSource,
    fields: HashMap<String, String>,
}

impl LogMessageEvent {
    pub fn new(source: LogSource, fields: HashMap<String, String>) -> Self {
        Self {
            source,
            fields,
        }
    }

    pub fn source(&self) -> &LogSource {
        &self.source
    }

    pub fn fields(&self) -> &HashMap<String, String> {
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
