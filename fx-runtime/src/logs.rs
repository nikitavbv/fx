use {
    std::collections::HashMap,
    tracing::{info, error},
    tokio::sync::mpsc,
    serde::Serialize,
    fx_runtime_common::{
        LogMessageEvent,
        LogSource as LogMessageEventLogSource,
    },
    crate::{cloud::FunctionId, error::LoggerError},
};

pub struct LogMessage {
    source: LogSource,
    level: LogLevel,
    fields: HashMap<String, String>,
}

impl LogMessage {
    pub fn new(source: LogSource, level: LogLevel, fields: HashMap<String, String>) -> Self {
        Self {
            source,
            level,
            fields,
        }
    }
}

impl Into<LogMessageEvent> for LogMessage {
    fn into(self) -> LogMessageEvent {
        LogMessageEvent::new(self.source.into(), self.fields)
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

pub struct RabbitMqLogger {
    tx: tokio::sync::mpsc::Sender<LogMessage>,
    _connection_task: tokio::task::JoinHandle<()>,
}

impl RabbitMqLogger {
    pub async fn new(uri: String, exchange: String) -> Result<Self, LoggerError> {
        let (tx, mut rx) = mpsc::channel(1024);

        let connection = lapin::Connection::connect(
            uri.as_str().into(),
            lapin::ConnectionProperties::default().with_connection_name("fx-log".into())
        ).await.map_err(|err| LoggerError::FailedToCreate { reason: format!("connection error: {err:?}") })?;

        let channel = connection.create_channel().await
            .map_err(|err| LoggerError::FailedToCreate { reason: format!("failed to create channel: {err:?}") })?;

        let connection_task = tokio::task::spawn(async move {
            info!("publishing logs to rabbitmq exchange: {exchange}");

            loop {
                let msg: LogMessage = match rx.recv().await {
                    Some(v) => v,
                    None => break,
                };

                let msg: LogMessageEvent = msg.into();

                let msg_encoded = match rmp_serde::to_vec(&msg) {
                    Ok(v) => v,
                    Err(err) => {
                        error!("failed to decode log message: {err:?}");
                        continue;
                    }
                };
                let routing_key = match msg.source() {
                    LogMessageEventLogSource::FxRuntime => "fx/runtime".to_owned(),
                    LogMessageEventLogSource::Function { id } => format!("fx/function/{id}")
                };

                let result = channel.basic_publish(
                    exchange.as_str(),
                    &routing_key,
                    lapin::options::BasicPublishOptions::default(),
                    &msg_encoded,
                    lapin::BasicProperties::default()
                ).await;

                if let Err(err) = result {
                    error!("failed to publish message to rabbitmq: {err:?}");
                }
            }
        });

        Ok(Self {
            tx,
            _connection_task: connection_task,
        })
    }
}

impl Logger for RabbitMqLogger {
    fn log(&self, message: LogMessage) {
        let tx = self.tx.clone();
        tokio::task::spawn(async move {
            if let Err(err) = tx.send(message).await {
                error!("failed to write log to rabbitmq: {err:?}");
            }
        });
    }
}
