use {
    std::sync::Arc,
    tracing::error,
    lapin::{Connection, ConnectionProperties, options::{BasicConsumeOptions, BasicNackOptions, BasicAckOptions}, types::FieldTable},
    futures::StreamExt,
    tokio::time::{sleep, Duration},
    fx_core::QueueMessage,
    crate::cloud::{Engine, ServiceId},
};

pub struct RabbitMqConsumer {
    engine: Arc<Engine>,
    amqp_addr: String,
    id: String,
    queue: String,
    function_id: ServiceId,
    rpc_method_name: String,
}

impl RabbitMqConsumer {
    pub fn new(engine: Arc<Engine>, amqp_addr: String, id: String, queue: String, function_id: String, rpc_method_name: String) -> Self {
        Self {
            engine,
            amqp_addr,
            id,
            queue,
            function_id: ServiceId::new(function_id),
            rpc_method_name,
        }
    }

    pub async fn run(&self) {
        let connection = Connection::connect(&self.amqp_addr, ConnectionProperties::default().with_connection_name(self.id.clone().into())).await.unwrap();

        let channel = connection.create_channel().await.unwrap();

        let mut consumer = channel
            .basic_consume(&self.queue, &self.id, BasicConsumeOptions::default(), FieldTable::default()).await.unwrap();

        while let Some(msg) = consumer.next().await {
            let msg = msg.unwrap();
            let message = QueueMessage {
                data: msg.data.clone(),
            };
            let result = self.engine.invoke_service::<QueueMessage, ()>(self.engine.clone(), &self.function_id, &self.rpc_method_name, message).await;
            if let Err(err) = result {
                error!("failed to invoke consumer: {err:?}");
                msg.nack(BasicNackOptions::default()).await.unwrap();
                sleep(Duration::from_secs(1)).await;
            } else {
                msg.ack(BasicAckOptions::default()).await.unwrap();
            }
        }
    }
}
