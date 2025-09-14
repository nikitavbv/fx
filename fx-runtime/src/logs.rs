use {
    std::{collections::HashMap, time::Duration},
    tracing::{info, error},
    tokio::sync::mpsc,
    serde::Serialize,
    tokio::time::sleep,
    fx_runtime_common::{
        LogMessageEvent,
        LogSource as LogMessageEventLogSource,
        utils::object_to_event_fields,
    },
    crate::{runtime::FunctionId, error::LoggerError},
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
        LogMessageEvent::new(self.source.into(), object_to_event_fields(self.fields).unwrap_or(HashMap::new()))
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

pub struct RabbitMqLogger {
    tx: tokio::sync::mpsc::Sender<LogMessageEvent>,
    _connection_task: tokio::task::JoinHandle<()>,
}

impl RabbitMqLogger {
    pub async fn new(uri: String, exchange: String) -> Result<Self, LoggerError> {
        let (tx, mut rx) = mpsc::channel(1024);
        let (mut _connection, mut channel) = Self::connect(&uri, &exchange).await?;

        let connection_task = tokio::task::spawn(async move {
            info!("publishing logs to rabbitmq exchange: {exchange}");

            loop {
                let msg: LogMessageEvent = match rx.recv().await {
                    Some(v) => v,
                    None => break,
                };

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
                    error!("failed to publish message to rabbitmq: {err:?}. Reconnecting...");
                    sleep(Duration::from_secs(1)).await;
                    let (new_connection, new_channel) = Self::connect(&uri, &exchange).await.unwrap();
                    _connection = new_connection;
                    channel = new_channel;
                }
            }
        });

        Ok(Self {
            tx,
            _connection_task: connection_task,
        })
    }

    async fn connect(uri: &String, exchange: &String) -> Result<(lapin::Connection, lapin::Channel), LoggerError> {
        let connection = lapin::Connection::connect(
            uri.as_str().into(),
            lapin::ConnectionProperties::default().with_connection_name("fx-log".into())
        ).await.map_err(|err| LoggerError::FailedToCreate { reason: format!("connection error: {err:?}") })?;

        let channel = connection.create_channel().await
            .map_err(|err| LoggerError::FailedToCreate { reason: format!("failed to create channel: {err:?}") })?;

        Ok((connection, channel))
    }
}

impl Logger for RabbitMqLogger {
    fn log(&self, message: LogMessageEvent) {
        let tx = self.tx.clone();
        tokio::task::spawn(async move {
            if let Err(err) = tx.send(message).await {
                error!("failed to write log to rabbitmq: {err:?}");
            }
        });
    }
}
