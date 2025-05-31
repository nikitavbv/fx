use {
    std::collections::HashMap,
    crate::cloud::ServiceId,
};

pub struct LogMessage {
    source: LogSource,
    fields: HashMap<String, String>,
}

impl LogMessage {
    pub fn new(source: LogSource, fields: HashMap<String, String>) -> Self {
        Self {
            source,
            fields,
        }
    }
}

pub enum LogSource {
    Function {
        id: String,
    },
    FxRuntime,
}

impl LogSource {
    pub fn function(function_id: &ServiceId) -> Self {
        Self::Function { id: function_id.into() }
    }
}

pub trait Logger {
    fn log(&self, message: LogMessage);
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
    fn log(&self, message: LogMessage) {
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
    fn log(&self, message: LogMessage) {
        let source = match message.source {
            LogSource::Function { id } => id,
            LogSource::FxRuntime => "fx".to_owned(),
        };
        println!("{source} | {:?}", message.fields);
    }
}
