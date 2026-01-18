use {
    std::time::Duration,
    tracing::{info, error},
    tokio::{sync::mpsc, time::sleep},
    crate::common::{
        LogMessageEvent,
        LogSource as LogMessageEventLogSource,
    },
    crate::runtime::{
        error::LoggerError,
        logs::Logger,
    },
};

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
