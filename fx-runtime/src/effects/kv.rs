use {
    std::time::Duration,
    thiserror::Error,
    futures::{stream::{BoxStream, Stream}, FutureExt, StreamExt},
};

pub(crate) struct KvSetRequest {
    pub(crate) key: Vec<u8>,
    pub(crate) value: Vec<u8>,
    pub(crate) nx: bool,
    pub(crate) px: Option<Duration>,
}

impl KvSetRequest {
    pub(crate) fn new(key: Vec<u8>, value: Vec<u8>) -> Self {
        Self {
            key,
            value,
            nx: false,
            px: None,
        }
    }

    pub(crate) fn with_nx(mut self, nx: bool) -> Self {
        self.nx = nx;
        self
    }

    pub(crate) fn with_px(mut self, px: Option<Duration>) -> Self {
        self.px = px;
        self
    }
}

#[derive(Debug, Error)]
pub(crate) enum KvSetError {
    #[error("key already exists")]
    AlreadyExists,
}

#[derive(Debug, Error)]
pub(crate) enum KvGetHandlerError {
    #[error("key not found")]
    KeyNotFound,
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding with requested name is not found")]
    BindingNotFound,
    #[error("invalid kv get request")]
    BadRequest,
    #[error("failed to read request")]
    FailedToReadRequest,
}

pub(crate) struct KvDelexRequest {
    pub(crate) key: Vec<u8>,
    pub(crate) ifeq: Vec<u8>,
}

#[derive(Debug, Error)]
pub(crate) enum KvDelexHandlerError {
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("failed to read request")]
    FailedToReadRequest,
    #[error("invalid kv delex request")]
    BadRequest,
    #[error("binding with requested name is not found")]
    BindingNotFound,
}

pub(crate) struct KvPublishRequest {
    pub(crate) channel: Vec<u8>,
    pub(crate) data: Vec<u8>,
}

#[derive(Debug, Error)]
pub(crate) enum KvPublishHandlerError {
    #[error("binding with requested name is not found")]
    BindingNotFound,
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("invalid kv publish request")]
    BadRequest,
    #[error("failed to read request")]
    FailedToReadRequest,
}

#[derive(Debug, Error)]
pub(crate) enum KvSubscriptionHandlerError {
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding with request name is not found")]
    BindingNotFound,
    #[error("invalid kv subscription request")]
    BadRequest,
    #[error("failed to read request")]
    FailedToReadRequest,
}

pub(crate) enum KvSubscriptionResource {
    Init(tokio::sync::oneshot::Receiver<flume::Receiver<Vec<u8>>>),
    Stream(BoxStream<'static, Vec<u8>>),
}

impl Stream for KvSubscriptionResource {
    type Item = Vec<u8>;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        let subscription = self.get_mut();
        match subscription {
            Self::Init(v) => match v.poll_unpin(cx) {
                std::task::Poll::Pending => return std::task::Poll::Pending,
                std::task::Poll::Ready(v) => {
                    let v = v.unwrap().into_stream();
                    *subscription = KvSubscriptionResource::Stream(v.boxed());
                    subscription.poll_next_unpin(cx)
                }
            },
            Self::Stream(v) => v.poll_next_unpin(cx)
        }
    }
}
