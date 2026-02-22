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

fn create_logger(logger: &LoggerConfig) -> BoxLogger {
    match logger {
        LoggerConfig::Stdout => BoxLogger::new(StdoutLogger::new()),
        LoggerConfig::Noop => BoxLogger::new(NoopLogger::new()),
        LoggerConfig::HttpLogger { endpoint } => BoxLogger::new(HttpLogger::new(endpoint.clone())),
        LoggerConfig::Custom(v) => BoxLogger::new(v.clone()),
    }
}

struct HttpLogger {
    client: reqwest::blocking::Client,
    endpoint: String,
}

impl HttpLogger {
    pub fn new(endpoint: String) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            endpoint,
        }
    }
}

impl Logger for HttpLogger {
    fn log(&self, message: LogMessageEvent) {
        let response = self.client.post(&self.endpoint)
            .header("content-type", "application/stream+json")
            .body(serde_json::to_vec(&HttpLogEvent {
                source: "fx".to_owned(),
                function_id: match message.source {
                    LogSource::FxRuntime => None,
                    LogSource::Function { id } => Some(id),
                },
                message: message.fields.get("message")
                    .map(|v| match v {
                        EventFieldValue::Text(v) => v.clone(),
                        other => format!("{other:?}"),
                    })
                    .unwrap_or(format!("fx log")),
                request_id: message.fields.get("request_id")
                    .map(|v| match v {
                        EventFieldValue::Text(v) => v.clone(),
                        other => format!("{other:?}"),
                    }),
                level: message.level,
                timestamp: chrono::Utc::now().to_rfc3339(),
                fields: message.fields.into_iter()
                    .map(|(key, value)| (
                        key.clone(),
                        map_log_value_to_serde_json(&value)
                    ))
                    .collect()
            }).unwrap())
            .send()
            .unwrap();
        if !response.status().is_success() {
            error!(status_code=response.status().as_u16(), "failed to send logs over http");
        }
    }
}

fn map_log_value_to_serde_json(value: &EventFieldValue) -> serde_json::Value {
    match value {
        EventFieldValue::Text(v) => serde_json::Value::String(v.clone()),
        EventFieldValue::F64(v) => serde_json::Value::Number(serde_json::Number::from_f64(*v).unwrap()),
        EventFieldValue::I64(v) => serde_json::Value::Number((*v).into()),
        EventFieldValue::U64(v) => serde_json::Value::Number((*v).into()),
        EventFieldValue::Object(v) => serde_json::Value::Object(
            v.iter()
                .map(|(key, value)| (key.clone(), map_log_value_to_serde_json(value)))
                .collect()
        )
    }
}

#[derive(Serialize)]
struct HttpLogEvent {
    fields: HashMap<String, serde_json::Value>,
    level: LogEventLevel,
    message: String,
    request_id: Option<String>,
    source: String,
    function_id: Option<String>,
    timestamp: String,
}
