use {
    std::time::Duration,
    tracing::{info, error},
    tokio::{sync::mpsc, time::sleep},
    fx_types::{capnp, events_capnp},
    crate::{
        common::{
            LogMessageEvent,
            LogSource as LogMessageEventLogSource,
        },
        runtime::{
            error::LoggerError,
            logs::Logger,
        },
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

                let msg_encoded = {
                    let mut msg_encoded = capnp::message::Builder::new_default();
                    let mut msg_event = msg_encoded.init_root::<events_capnp::fx_log_event::Builder>();
                    let mut msg_source = msg_event.reborrow().init_source().init_source();
                    match &msg.source {
                        crate::common::LogSource::Function { id } => msg_source.set_function(&id),
                        crate::common::LogSource::FxRuntime => msg_source.set_runtime(()),
                    };

                    msg_event.set_event_type(match &msg.event_type {
                        crate::common::LogEventType::Begin => events_capnp::LogEventType::Begin,
                        crate::common::LogEventType::End => events_capnp::LogEventType::End,
                        crate::common::LogEventType::Instant => events_capnp::LogEventType::Instant,
                    });

                    msg_event.set_level(match &msg.level {
                        crate::common::LogEventLevel::Trace => events_capnp::LogLevel::Trace,
                        crate::common::LogEventLevel::Debug => events_capnp::LogLevel::Debug,
                        crate::common::LogEventLevel::Info => events_capnp::LogLevel::Info,
                        crate::common::LogEventLevel::Warn => events_capnp::LogLevel::Warn,
                        crate::common::LogEventLevel::Error => events_capnp::LogLevel::Error,
                    });

                    fn write_log_event_object(mut builder: capnp::struct_list::Builder<'_, events_capnp::log_event_field::Owned>, object: impl Iterator<Item = (u32, (String, crate::common::EventFieldValue))>) {
                        for (index, (key, value)) in object {
                            let mut msg_field = builder.reborrow().get(index as u32);
                            msg_field.set_key(key);
                            let mut msg_field_value = msg_field.init_value();
                            match value {
                                crate::common::EventFieldValue::Text(v) => msg_field_value.set_text(&v),
                                crate::common::EventFieldValue::U64(v) => msg_field_value.set_u64(v),
                                crate::common::EventFieldValue::I64(v) => msg_field_value.set_i64(v),
                                crate::common::EventFieldValue::F64(v) => msg_field_value.set_f64(v),
                                crate::common::EventFieldValue::Object(v) => {
                                    let msg_fields = msg_field_value.init_object(v.len() as u32);
                                    write_log_event_object(msg_fields, v.into_iter().enumerate().map(|(index, (key, value))| (index as u32, (key, (*value).clone()))));
                                }
                            }
                        }
                    }

                    let msg_fields = msg_event.init_fields(msg.fields.len() as u32);
                    write_log_event_object(msg_fields, msg.fields().iter().enumerate().map(|v| (v.0 as u32, (v.1.0.to_owned(), v.1.1.clone()))));

                    capnp::serialize::write_message_to_words(&msg_encoded)
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
