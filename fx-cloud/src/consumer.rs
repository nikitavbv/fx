use {
    std::sync::Arc,
    lapin::{Connection, ConnectionProperties, options::BasicConsumeOptions, types::FieldTable},
    futures::StreamExt,
    crate::cloud::Engine,
};

pub struct RabbitMqConsumer {
    engine: Arc<Engine>,
    amqp_addr: String,
    id: String,
    queue: String,
    function_id: String,
    rpc_method_name: String,
}

impl RabbitMqConsumer {
    pub fn new(engine: Arc<Engine>, amqp_addr: String, id: String, queue: String, function_id: String, rpc_method_name: String) -> Self {
        Self {
            engine,
            amqp_addr,
            id,
            queue,
            function_id,
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
            // TODO: handle message
        }
    }
}
