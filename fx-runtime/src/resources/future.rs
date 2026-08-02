use {
    std::{task::Poll, rc::Rc, pin::Pin},
    futures::{future::LocalBoxFuture, FutureExt},
    crate::{
        function::instance::{
            FunctionInstance,
            background_task_poll::BackgroundTaskPollError,
            function_response_poll::FunctionResponsePollError,
        },
        resources::FunctionResourceId,
        triggers::http::HttpBody,
    },
};

pub(crate) struct FunctionBackgroundTask {
    inner: LocalBoxFuture<'static, Result<Poll<()>, BackgroundTaskPollError>>,
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

    fn start_new_poll_call(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> LocalBoxFuture<'static, Result<Poll<()>, BackgroundTaskPollError>> {
        async move {
            let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
            instance.background_task_poll(&resource_id, waker).await
        }.boxed_local()
    }
}

impl Future for FunctionBackgroundTask {
    type Output = Result<(), BackgroundTaskPollError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match self.inner.poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => {
                if err == BackgroundTaskPollError::FunctionCrashed || err == BackgroundTaskPollError::FunctionPanicked {
                    *self.instance.has_panicked.borrow_mut() = true;
                }
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

pub(crate) struct FunctionResponseFuture {
    inner: LocalBoxFuture<'static, Poll<Result<http::Response<HttpBody>, FunctionResponsePollError>>>,
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
}

impl FunctionResponseFuture {
    pub(crate) fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: Self::start_new_poll_call(instance.clone(), resource_id.clone()),
            instance,
            resource_id,
        }
    }

    fn start_new_poll_call(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> LocalBoxFuture<'static, Poll<Result<http::Response<HttpBody>, FunctionResponsePollError>>> {
        async move {
            let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
            instance.function_response_poll(&resource_id, waker).await
        }.boxed_local()
    }
}

impl Future for FunctionResponseFuture {
    type Output = Result<http::Response<HttpBody>, FunctionResponsePollError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match self.inner.poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Poll::Pending) => {
                self.inner = Self::start_new_poll_call(self.instance.clone(), self.resource_id.clone());
                Poll::Pending
            },
            Poll::Ready(Poll::Ready(Err(err))) => {
                if err == FunctionResponsePollError::FunctionPanicked || err == FunctionResponsePollError::FunctionCrashed {
                    *self.instance.has_panicked.borrow_mut() = true;
                }
                Poll::Ready(Err(err))
            },
            Poll::Ready(Poll::Ready(Ok(v))) => Poll::Ready(Ok(v)),
        }
    }
}
