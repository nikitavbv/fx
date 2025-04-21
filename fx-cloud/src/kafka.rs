use {
    rdkafka::{
        client::ClientContext,
        consumer::{ConsumerContext, StreamConsumer, Consumer, CommitMode},
        ClientConfig,
        Message,
    },
    crate::{FxCloud, ServiceId},
};

struct CustomContext;
impl ClientContext for CustomContext {}
impl ConsumerContext for CustomContext {}

impl FxCloud {
    #[allow(dead_code)]
    pub async fn run_kafka(&self, bootstrap_servers: &str, topic: &str, service_id: &ServiceId, function_name: &str) {
        let consumer: StreamConsumer<CustomContext> = ClientConfig::new()
            .set("group.id", "fx")
            .set("bootstrap.servers", bootstrap_servers)
            .create_with_context(CustomContext)
            .unwrap();
        consumer.subscribe(&[topic]).unwrap();

        loop {
            let msg = consumer.recv().await.unwrap();
            let payload = msg.payload().unwrap();
            self.engine.clone().invoke_service_raw(self.engine.clone(), service_id, function_name, payload.to_vec());
            consumer.commit_message(&msg, CommitMode::Async).unwrap();
        }
    }
}
