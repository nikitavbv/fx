impl SerializeResource for Vec<u8> {
    fn serialize(self) -> Vec<u8> {
        self
    }
}

enum FutureResource<T> {
    Future(BoxFuture<'static, T>),
    Ready(T),
}

struct FunctionFuture {
    inner: LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>>,
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
}

impl FunctionFuture {
    fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
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
                        FunctionFuturePollError::FunctionPanicked => FunctionFutureError::FunctionPanicked,
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
