use {
    std::rc::Rc,
    futures::future::{LocalBoxFuture, FutureExt},
    crate::{resources::FunctionResourceId, triggers::http::HttpBody, function::instance::FunctionInstance},
};

pub(crate) struct FunctionHttpResponseFuture {
    inner: LocalBoxFuture<'static, http::Response<HttpBody>>,
}

impl FunctionHttpResponseFuture {
    pub fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: async move {
                todo!()
            }.boxed_local()
        }
    }
}

impl Future for FunctionHttpResponseFuture {
    type Output = http::Response<HttpBody>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        todo!()
    }
}
