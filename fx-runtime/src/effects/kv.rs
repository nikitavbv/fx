use {
    std::{time::Duration, collections::HashMap},
    thiserror::Error,
    futures::{stream::{BoxStream, Stream}, FutureExt, StreamExt},
    axum::{routing::RouterIntoService, Router},
    fx_types::{capnp, abi_kv_capnp},
    crate::{
        tasks::kv::KvMessage,
        triggers::http::HttpBody,
        definitions::bindings::KvBindingConfig,
    },
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
pub(crate) enum KvSetHandlerError {
    #[error("key already exists")]
    AlreadyExists,
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding with requested name is not found")]
    BindingNotFound,
    #[error("failed to read request")]
    FailedToReadRequest,
    #[error("invalid kv set request")]
    BadRequest,
}

impl From<KvSetError> for KvSetHandlerError {
    fn from(err: KvSetError) -> Self {
        match err {
            KvSetError::AlreadyExists => Self::AlreadyExists,
        }
    }
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

pub(crate) fn create_service(sender: flume::Sender<KvMessage>, bindings: HashMap<String, KvBindingConfig>) -> RouterIntoService<HttpBody> {
    Router::new()
        .route("/get", axum::routing::post(handle_kv_get))
        .route("/set", axum::routing::post(handle_kv_set))
        .layer(axum::Extension(sender))
        .layer(axum::Extension(bindings))
        .into_service()
}

async fn handle_kv_get(
    axum::Extension(sender): axum::Extension<flume::Sender<KvMessage>>,
    axum::Extension(bindings): axum::Extension<HashMap<String, KvBindingConfig>>,
    body: axum::body::Bytes,
) -> impl axum::response::IntoResponse {
    async fn handler(kv_tx: flume::Sender<KvMessage>, bindings: &HashMap<String, KvBindingConfig>, mut request: &[u8]) -> Result<Option<Vec<u8>>, KvGetHandlerError> {
        let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
        let request = request_reader.get_root::<abi_kv_capnp::kv_get_request::Reader>().unwrap();

        let binding = request.get_binding().map_err(|_| KvGetHandlerError::BadRequest)?;
        let binding = str::from_utf8(&binding.as_bytes()).map_err(|_| KvGetHandlerError::BadRequest)?;
        let namespace = bindings.get(binding).ok_or(KvGetHandlerError::BindingNotFound)?.namespace.clone();

        let key = request.get_key().map_err(|_| KvGetHandlerError::BadRequest)?.to_vec();

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: crate::tasks::kv::KvOperation::Get { key, result: result_tx },
        }).await.map_err(|_| KvGetHandlerError::RuntimeShutdown)?;

        result_rx.await.map_err(|_| KvGetHandlerError::RuntimeShutdown)
    }

    let response = handler(sender, &bindings, body.as_ref()).await;

    let mut message = capnp::message::Builder::new_default();
    let message_response = message.init_root::<abi_kv_capnp::kv_get_response::Builder>();
    let mut message_response = message_response.init_response();

    match response {
        Ok(Some(v)) => message_response.set_value(&v),
        Ok(None) | Err(KvGetHandlerError::KeyNotFound) => message_response.set_key_not_found(()),
        Err(KvGetHandlerError::RuntimeShutdown) => message_response.set_runtime_shutdown(()),
        Err(KvGetHandlerError::BindingNotFound) => message_response.set_binding_not_found(()),
        Err(KvGetHandlerError::BadRequest) => message_response.set_bad_request(()),
        Err(KvGetHandlerError::FailedToReadRequest) => message_response.set_failed_to_read_request(()),
    }

    capnp::serialize::write_message_to_words(&message)
}

async fn handle_kv_set(
) -> impl axum::response::IntoResponse {
    ()
}
