use {
    std::cell::RefCell,
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
    function_invoke_round_robin_counter: RefCell<u64>,
}

impl WorkersController {
    pub fn new(workers_tx: Vec<flume::Sender<WorkerMessage>>) -> Self {
        Self {
            workers_tx,
            function_invoke_round_robin_counter: RefCell::new(0),
        }
    }

    pub(crate) fn function_remove(&self, function_id: &FunctionId) -> impl Future<Output = Result<(), FunctionRemoveError>> {
        let workers_tx = self.workers_tx.clone();
        async move {
            let subtasks = FuturesUnordered::new();

            for worker in workers_tx {
                subtasks.push(async move {
                    let (on_ready_tx, on_ready_rx) = oneshot::channel();
                    worker.send_async(WorkerMessage::RemoveFunction {
                        function_id: function_id.clone(),
                        on_ready: Some(on_ready_tx),
                    }).await.unwrap();
                    on_ready_rx.await
                        .map_err(|_| FunctionRemoveError::WorkerShutdown)
                });
            }

            for result in subtasks.collect::<Vec<_>>().await {
                result?;
            }
            Ok(())
        }
    }

    pub(crate) async fn function_invoke(&self, function_id: FunctionId, req: FetchRequestHeader) -> Result<(), WorkersControllerFunctionInvokeError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.workers_tx.get(*self.function_invoke_round_robin_counter.borrow() as usize).unwrap().send_async(WorkerMessage::FunctionInvoke { function_id, header: req, response_tx }).await.unwrap();
        {
            let mut counter = self.function_invoke_round_robin_counter.borrow_mut();
            // TODO: maybe counter is not needed and this can be replaced with a single flume channel shared by all threads?
            *counter = (*counter + 1) % (self.workers_tx.len() as u64);
        }
        response_rx.await
            .map_err(|_| WorkersControllerFunctionInvokeError::WorkerShutdown)?
            .map_err(WorkersControllerFunctionInvokeError::from)
    }
}

#[derive(Debug, Error)]
pub(crate) enum FunctionRemoveError {
    #[error("failed to remove function because worker shut down")]
    WorkerShutdown,
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
