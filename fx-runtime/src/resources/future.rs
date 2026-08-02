use {
    std::{task::Poll, rc::Rc, pin::Pin},
    futures::{future::LocalBoxFuture, FutureExt},
    crate::{
        function::{
            instance::{FunctionFuturePollError, FunctionInstance, WasmFunctionCallError},
            deployment::FunctionFutureError,
        },
        resources::FunctionResourceId,
    },
};

pub(crate) struct FunctionFuture {
    inner: LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>>,
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
}

impl FunctionFuture {
    pub(crate) fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: Self::start_new_poll_call(instance.clone(), resource_id.clone()),
            instance,
            resource_id,
        }
    }

    fn start_new_poll_call(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>> {
        async move {
            let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
            instance.future_poll(&resource_id, waker).await
        }.boxed_local()
    }
}

impl Future for FunctionFuture {
    type Output = Result<FunctionResourceId, FunctionFutureError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        match self.inner.poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(v) => {
                let poll = match v {
                    Ok(v) => v,
                    Err(err) => return Poll::Ready(Err(match err {
                        FunctionFuturePollError::FunctionPanicked => {
                            *self.instance.has_panicked.borrow_mut() = true;
                            FunctionFutureError::FunctionPanicked
                        },
                    })),
                };

                match poll {
                    Poll::Pending => {
                        self.inner = Self::start_new_poll_call(self.instance.clone(), self.resource_id.clone());
                        Poll::Pending
                    },
                    Poll::Ready(_) => Poll::Ready(Ok(self.resource_id.clone()))
                }
            },
        }
    }
}

pub struct FunctionUnitFuture {
    inner: LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>>,
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
}

impl FunctionUnitFuture {
    pub(crate) fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: Self::start_new_poll_call(instance.clone(), resource_id.clone()),
            instance,
            resource_id,
        }
    }

    fn start_new_poll_call(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>> {
        async move {
            let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
            instance.future_poll(&resource_id, waker).await
        }.boxed_local()
    }
}

impl Future for FunctionUnitFuture {
    type Output = Result<(), FunctionFutureError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match self.inner.poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(v) => {
                let poll = match v {
                    Ok(v) => v,
                    Err(err) => return Poll::Ready(Err(match err {
                        FunctionFuturePollError::FunctionPanicked => {
                            *self.instance.has_panicked.borrow_mut() = true;
                            FunctionFutureError::FunctionPanicked
                        }
                    })),
                };

                match poll {
                    Poll::Pending => {
                        self.inner = Self::start_new_poll_call(self.instance.clone(), self.resource_id.clone());
                        Poll::Pending
                    },
                    Poll::Ready(_) => Poll::Ready(Ok(())),
                }
            }
        }
    }
}

pub(crate) struct FunctionBackgroundTask {
    inner: LocalBoxFuture<'static, Result<Poll<()>, WasmFunctionCallError>>,
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
}

impl FunctionBackgroundTask {
    pub(crate) fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: Self::start_new_poll_call(instance.clone(), resource_id.clone()),
            instance,
            resource_id,
        }
    }

    fn start_new_poll_call(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> LocalBoxFuture<'static, Result<Poll<()>, WasmFunctionCallError>> {
        async move {
            let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
            instance.background_task_poll(&resource_id, waker).await
        }.boxed_local()
    }
}

impl Future for FunctionBackgroundTask {
    type Output = Result<(), WasmFunctionCallError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match self.inner.poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => {
                *self.instance.has_panicked.borrow_mut() = true;
                Poll::Ready(Err(err))
            },
            Poll::Ready(Ok(Poll::Pending)) => {
                self.inner = Self::start_new_poll_call(self.instance.clone(), self.resource_id.clone());
                Poll::Pending
            },
            Poll::Ready(Ok(Poll::Ready(_))) => Poll::Ready(Ok(())),
        }
    }
}
