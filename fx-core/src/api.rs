use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
};

#[derive(Serialize, Deserialize)]
pub enum FxApiRequest {
    Log(LogRequest),
}

#[derive(Serialize, Deserialize)]
pub enum FxApiResponse {
    Empty,
}

#[derive(Serialize, Deserialize)]
pub struct LogRequest {
    pub level: LogLevel,
    pub fields: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
