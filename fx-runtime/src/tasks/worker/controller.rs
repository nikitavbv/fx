use {
    tokio::sync::oneshot,
    futures::{stream::FuturesUnordered, StreamExt},
    crate::function::FunctionId,
    super::messages::WorkerMessage,
};

pub(crate) struct WorkersController {
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
}

impl WorkersController {
    pub fn new(workers_tx: Vec<flume::Sender<WorkerMessage>>) -> Self {
        Self {
            workers_tx,
        }
    }

    pub(crate) async fn function_remove(&self, function_id: &FunctionId) {
        let subtasks = FuturesUnordered::new();

        for worker in &self.workers_tx {
            subtasks.push(async {
                let (on_ready_tx, on_ready_rx) = oneshot::channel();
                worker.send_async(WorkerMessage::RemoveFunction {
                    function_id: function_id.clone(),
                    on_ready: Some(on_ready_tx),
                }).await.unwrap();
                on_ready_rx.await.unwrap();
            });
        }

        subtasks.collect::<Vec<_>>().await;
    }
}
