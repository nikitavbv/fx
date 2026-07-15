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

pub(crate) enum KvGetResponse {
    KeyNotFound,
    Ok(Vec<u8>),
}

pub(crate) struct KvDelexRequest {
    pub(crate) key: Vec<u8>,
    pub(crate) ifeq: Vec<u8>,
}

pub(crate) struct KvPublishRequest {
    pub(crate) channel: Vec<u8>,
    pub(crate) data: Vec<u8>,
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
