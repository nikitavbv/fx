use {
    std::collections::HashMap,
    tracing::info,
    tokio::sync::mpsc,
    serde::Serialize,
    crate::cloud::ServiceId,
};

#[derive(Serialize)]
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

#[derive(Serialize)]
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

pub struct RabbitMqLogger {
    tx: tokio::sync::mpsc::Sender<LogMessage>,
    _connection_task: tokio::task::JoinHandle<()>,
}

impl RabbitMqLogger {
    pub fn new(uri: String, exchange: String) -> Self {
        let (tx, mut rx) = mpsc::channel(1024);
        let connection_task = tokio::task::spawn(async move {
            info!("publishing logs to rabbitmq exchange: {exchange}");
            let connection = lapin::Connection::connect(
                uri.as_str().into(),
                lapin::ConnectionProperties::default().with_connection_name("fx-log".into())
            ).await.unwrap();

            let channel = connection.create_channel().await.unwrap();

            loop {
                let msg: LogMessage = match rx.recv().await {
                    Some(v) => v,
                    None => break,
                };

                let msg_encoded = rmp_serde::to_vec(&msg).unwrap();
                let routing_key = match msg.source {
                    LogSource::FxRuntime => "fx/runtime".to_owned(),
                    LogSource::Function { id } => format!("fx/function/{id}")
                };

                channel.basic_publish(
                    exchange.as_str(),
                    &routing_key,
                    lapin::options::BasicPublishOptions::default(),
                    &msg_encoded,
                    lapin::BasicProperties::default()
                ).await.unwrap();
            }
        });

        Self {
            tx,
            _connection_task: connection_task,
        }
    }
}

impl Logger for RabbitMqLogger {
    fn log(&self, message: LogMessage) {
        self.tx.blocking_send(message).unwrap();
    }
}
