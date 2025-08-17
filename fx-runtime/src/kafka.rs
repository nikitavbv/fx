use {
    std::time::Duration,
    tracing::error,
    rdkafka::{
        client::ClientContext,
        consumer::{ConsumerContext, StreamConsumer, Consumer, CommitMode},
        ClientConfig,
        Message,
    },
    tokio::time::sleep,
    crate::{FxCloud, FunctionId},
};

struct CustomContext;
impl ClientContext for CustomContext {}
impl ConsumerContext for CustomContext {}

impl FxCloud {
    #[allow(dead_code)]
    pub async fn run_kafka(&self, bootstrap_servers: &str, topic: &str, service_id: &FunctionId, function_name: &str) {
        let consumer: StreamConsumer<CustomContext> = ClientConfig::new()
            .set("group.id", "fx")
            .set("bootstrap.servers", bootstrap_servers)
            .create_with_context(CustomContext)
            .unwrap();
        consumer.subscribe(&[topic]).unwrap();

        loop {
            let msg = consumer.recv().await.unwrap();
            let payload = msg.payload().unwrap();
            match self.invoke_service_raw(service_id, function_name, payload.to_vec()) {
                Ok(v) => {
                    match v.await {
                        Ok(_v) => {
                            if let Err(err) = consumer.commit_message(&msg, CommitMode::Async) {
                                error!("failed to commit kafka message: {err:?}");
                                sleep(Duration::from_secs(1)).await;
                                continue;
                            }
                        },
                        Err(err) => {
                            // TODO: these errors should be reported and counted
                            error!("failed to handle kafka message, error when running function: {err:?}");
                            sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                },
                Err(err) => {
                    // TODO: these errors should be reported and counted
                    error!("failed to handle kafka message, error when invoking function: {err:?}");
                    sleep(Duration::from_secs(1)).await;
                    continue;
                }
            }
        }
    }
}
