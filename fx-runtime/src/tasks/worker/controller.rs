use {
    tokio::sync::oneshot,
    futures::{stream::FuturesUnordered, StreamExt},
    crate::{
        function::FunctionId,
        triggers::http::FetchRequestHeader,
    },
    super::messages::WorkerMessage,
};

#[derive(Clone)]
pub(crate) struct WorkersController {
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
    function_invoke_round_robin_counter: u64,
}

impl WorkersController {
    pub fn new(workers_tx: Vec<flume::Sender<WorkerMessage>>) -> Self {
        Self {
            workers_tx,
            function_invoke_round_robin_counter: 0,
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

    pub(crate) async fn function_invoke(&mut self, function_id: FunctionId, req: FetchRequestHeader) {
        let (response_tx, response_rx) = flume::unbounded();
        self.workers_tx.get(self.function_invoke_round_robin_counter as usize).unwrap().send_async(WorkerMessage::FunctionInvoke { function_id, header: req, response_tx });
        self.function_invoke_round_robin_counter = (self.function_invoke_round_robin_counter + 1) % (self.workers_tx.len() as u64);
        response_rx.recv_async().await;
    }
}
