use {
    std::{collections::HashMap, sync::Arc},
    tracing::{info, error},
    tokio::sync::mpsc,
    serde::Serialize,
    tokio::time::sleep,
    fx_runtime_common::{
        LogSource as LogMessageEventLogSource,
        LogEventType as LogMessageEventLogEventType,
        utils::object_to_event_fields,
    },
    crate::{runtime::FunctionId, error::LoggerError},
};

pub use fx_runtime_common::{LogMessageEvent, EventFieldValue, LogEventType};

pub struct LogMessage {
    source: LogSource,
    event_type: LogEventType,
    level: LogLevel,
    fields: HashMap<String, String>,
}

impl LogMessage {
    pub fn new(source: LogSource, event_type: LogEventType, level: LogLevel, fields: HashMap<String, String>) -> Self {
        Self {
            source,
            event_type,
            level,
            fields,
        }
    }
}

impl Into<LogMessageEvent> for LogMessage {
    fn into(self) -> LogMessageEvent {
        LogMessageEvent::new(
            self.source.into(),
            self.event_type.into(),
            object_to_event_fields(self.fields).unwrap_or(HashMap::new())
        )
    }
}

#[derive(Serialize)]
pub enum LogSource {
    Function {
        id: String,
    },
    FxRuntime,
}

impl LogSource {
    pub fn function(function_id: &FunctionId) -> Self {
        Self::Function { id: function_id.into() }
    }
}

impl Into<LogMessageEventLogSource> for LogSource {
    fn into(self) -> LogMessageEventLogSource {
        match self {
            Self::Function { id } => LogMessageEventLogSource::Function { id },
            Self::FxRuntime => LogMessageEventLogSource::FxRuntime,
        }
    }
}

#[derive(Serialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

pub trait Logger {
    fn log(&self, message: LogMessageEvent);
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
            LogMessageEventLogSource::Function { id } => id,
            LogMessageEventLogSource::FxRuntime => "fx".to_owned(),
        };
        println!("{source} | {:?}", message.fields);
    }
}

impl<T: Logger> Logger for Arc<T> {
    fn log(&self, message: LogMessageEvent) {
        self.as_ref().log(message);
    }
}
