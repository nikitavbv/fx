use {
    std::{collections::HashMap, sync::Arc},
    serde::{Serialize, Deserialize},
};

pub trait Logger {
    fn log(&self, message: LogMessageEvent);
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogMessageEvent {
    pub source: LogSource,
    pub event_type: LogEventType,
    pub level: LogEventLevel,
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
    pub fn new(source: LogSource, event_type: LogEventType, level: LogEventLevel, fields: HashMap<String, EventFieldValue>) -> Self {
        Self {
            source,
            event_type,
            level,
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

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub enum LogEventType {
    Begin,
    End,
    Instant,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LogSource {
    Function {
        id: String,
    },
    FxRuntime,
}

impl LogSource {
    pub fn function(function_id: impl Into<String>) -> Self {
        Self::Function { id: function_id.into() }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LogEventLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

pub struct BoxLogger {
    inner: Box<dyn Logger + Send + Sync>,
}

impl BoxLogger {
    pub fn new<T: Logger + Send + Sync + 'static>(inner: T) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }
}

impl Logger for BoxLogger {
    fn log(&self, message: LogMessageEvent) {
        self.inner.log(message)
    }
}

pub struct StdoutLogger {}

impl StdoutLogger {
    pub fn new() -> Self {
        Self {}
    }
}

impl Logger for StdoutLogger {
    fn log(&self, message: LogMessageEvent) {
        let source = match message.source {
            LogSource::Function { id } => id,
            LogSource::FxRuntime => "fx".to_owned(),
        };
        println!("{source} | {:?}", message.fields);
    }
}

pub struct NoopLogger {}

impl NoopLogger {
    pub fn new() -> Self {
        Self {}
    }
}

impl Logger for NoopLogger {
    fn log(&self, _message: LogMessageEvent) {
    }
}

impl<T: Logger> Logger for Arc<T> {
    fn log(&self, message: LogMessageEvent) {
        self.as_ref().log(message);
    }
}
