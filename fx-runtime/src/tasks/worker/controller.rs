use {
    tokio::sync::oneshot,
    futures::{stream::FuturesUnordered, StreamExt},
    send_wrapper::SendWrapper,
    thiserror::Error,
    crate::{
        function::FunctionId,
        triggers::http::{FetchRequestHeader, FunctionResponse},
        resources::serialize::SerializedFunctionResource,
        tasks::worker::messages::FunctionInvokeError,
    },
    super::messages::{WorkerMessage, WorkerLocalMessage},
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

    pub(crate) async fn function_invoke(&mut self, function_id: FunctionId, req: FetchRequestHeader) -> Result<(), WorkersControllerFunctionInvokeError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.workers_tx.get(self.function_invoke_round_robin_counter as usize).unwrap().send_async(WorkerMessage::FunctionInvoke { function_id, header: req, response_tx }).await.unwrap();
        self.function_invoke_round_robin_counter = (self.function_invoke_round_robin_counter + 1) % (self.workers_tx.len() as u64);
        response_rx.await
            .map_err(|_| WorkersControllerFunctionInvokeError::WorkerShutdown)?
            .map_err(WorkersControllerFunctionInvokeError::from)
    }
}

#[derive(Debug, Error)]
pub(crate) enum WorkersControllerFunctionInvokeError {
    #[error("failed to run function because worker shut down")]
    WorkerShutdown,

    #[error("target function not found")]
    NotFound,

    #[error("function panicked")]
    FunctionPanicked,
}

impl From<FunctionInvokeError> for WorkersControllerFunctionInvokeError {
    fn from(error: FunctionInvokeError) -> Self {
        match error {
            FunctionInvokeError::NotFound => Self::NotFound,
            FunctionInvokeError::FunctionPanicked => Self::FunctionPanicked,
        }
    }
}

#[derive(Clone)]
pub(crate) struct LocalWorkerController {
    self_tx: SendWrapper<async_unsync::unbounded::UnboundedSender<WorkerLocalMessage>>,
}

impl LocalWorkerController {
    pub(crate) fn new(self_tx: async_unsync::unbounded::UnboundedSender<WorkerLocalMessage>) -> Self {
        Self {
            self_tx: SendWrapper::new(self_tx),
        }
    }

    pub(crate) fn invoke_function(&self, function_id: FunctionId, header: FetchRequestHeader) -> async_unsync::oneshot::Receiver<SerializedFunctionResource<FunctionResponse>> {
        let (response_tx, response_rx) = async_unsync::oneshot::channel().into_split();

        self.self_tx.send(WorkerLocalMessage::FunctionInvoke {
            function_id,
            header,
            response_tx,
        }).unwrap();

        response_rx
    }
}
