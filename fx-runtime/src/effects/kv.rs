use {
    std::{time::Duration, collections::HashMap},
    tokio::sync::oneshot,
    thiserror::Error,
    futures::{FutureExt, StreamExt},
    bytes::Bytes,
    http_body_util::BodyExt,
    fx_types::{capnp, abi_kv_capnp},
    crate::{
        effects::fetch::HttpStreamError,
        function::instance::FunctionInstanceState,
        tasks::kv::{KvMessage, KvOperation},
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
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding with requested name is not found")]
    BindingNotFound,
    #[error("invalid kv get request")]
    BadRequest,
}

pub(crate) struct KvDelexRequest {
    pub(crate) key: Vec<u8>,
    pub(crate) ifeq: Vec<u8>,
}

#[derive(Debug, Error)]
pub(crate) enum KvDelexHandlerError {
    #[error("runtime is being shut down")]
    RuntimeShutdown,
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
}

#[derive(Debug, Error)]
pub(crate) enum KvSubscriptionHandlerError {
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding with request name is not found")]
    BindingNotFound,
    #[error("invalid kv subscription request")]
    BadRequest,
}

pub(crate) fn handle_kv_request(state: &FunctionInstanceState, req: http::Request<HttpBody>) -> futures::future::LocalBoxFuture<'static, http::Response<HttpBody>> {
    match req.uri().path() {
        "/get" => handle_kv_get(state.runtime_services.kv.clone(), state.bindings.kv.clone(), req).boxed_local(),
        "/set" => handle_kv_set(state.runtime_services.kv.clone(), state.bindings.kv.clone(), req).boxed_local(),
        "/publish" => handle_kv_publish(state.runtime_services.kv.clone(), state.bindings.kv.clone(), req).boxed_local(),
        "/delex_ifeq" => handle_kv_delex_ifeq(state.runtime_services.kv.clone(), state.bindings.kv.clone(), req).boxed_local(),
        "/subscribe" => handle_kv_subscribe(state.runtime_services.kv.clone(), state.bindings.kv.clone(), req).boxed_local(),
        _other => {
            let mut response = http::Response::new(HttpBody::for_bytes("not found.\n".into()));
            *response.status_mut() = http::StatusCode::NOT_FOUND;
            std::future::ready(response).boxed_local()
        },
    }
}

async fn handle_kv_get(kv_tx: flume::Sender<KvMessage>, bindings: HashMap<String, KvBindingConfig>, req: http::Request<HttpBody>) -> http::Response<HttpBody> {
    async fn handler(kv_tx: flume::Sender<KvMessage>, bindings: &HashMap<String, KvBindingConfig>, mut request: &[u8]) -> Result<Option<Vec<u8>>, KvGetHandlerError> {
        let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
        let request = request_reader.get_root::<abi_kv_capnp::kv_get_request::Reader>().unwrap();

        let binding = request.get_binding().map_err(|_| KvGetHandlerError::BadRequest)?;
        let binding = str::from_utf8(&binding.as_bytes()).map_err(|_| KvGetHandlerError::BadRequest)?;
        let namespace = bindings.get(binding).ok_or(KvGetHandlerError::BindingNotFound)?.namespace.clone();

        let key = request.get_key().map_err(|_| KvGetHandlerError::BadRequest)?.to_vec();

        let (result_tx, result_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Get { key, result: result_tx },
        }).await.map_err(|_| KvGetHandlerError::RuntimeShutdown)?;

        result_rx.await.map_err(|_| KvGetHandlerError::RuntimeShutdown)
    }

    let bytes: Bytes = req.into_body().collect().await.unwrap().to_bytes();
    let kv_get_response = handler(kv_tx, &bindings, bytes.as_ref()).await;

    let mut message = capnp::message::Builder::new_default();
    let message_response = message.init_root::<abi_kv_capnp::kv_get_response::Builder>();
    let mut message_response = message_response.init_response();

    match kv_get_response {
        Ok(Some(v)) => message_response.set_value(&v),
        Ok(None) => message_response.set_key_not_found(()),
        Err(KvGetHandlerError::RuntimeShutdown) => message_response.set_runtime_shutdown(()),
        Err(KvGetHandlerError::BindingNotFound) => message_response.set_binding_not_found(()),
        Err(KvGetHandlerError::BadRequest) => message_response.set_bad_request(()),
    }

    http::Response::new(HttpBody::for_bytes(capnp::serialize::write_message_to_words(&message).into()))
}

async fn handle_kv_set(kv_tx: flume::Sender<KvMessage>, bindings: HashMap<String, KvBindingConfig>, req: http::Request<HttpBody>) -> http::Response<HttpBody> {
    async fn handler(kv_tx: flume::Sender<KvMessage>, bindings: &HashMap<String, KvBindingConfig>, mut request: &[u8]) -> Result<(), KvSetHandlerError> {
        let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
        let request = request_reader.get_root::<abi_kv_capnp::kv_set_request::Reader>().unwrap();

        let binding = request.get_binding().map_err(|_| KvSetHandlerError::BadRequest)?;
        let binding = str::from_utf8(&binding.as_bytes()).map_err(|_| KvSetHandlerError::BadRequest)?;
        let namespace = bindings.get(binding).ok_or(KvSetHandlerError::BindingNotFound)?.namespace.clone();

        let key = request.get_key().map_err(|_| KvSetHandlerError::BadRequest)?.to_vec();
        let value = request.get_value().map_err(|_| KvSetHandlerError::BadRequest)?.to_vec();
        let nx = request.get_nx();
        let px = request.get_px();

        let req = KvSetRequest::new(key, value)
            .with_nx(nx != 0)
            .with_px(if px > 0 { Some(Duration::from_millis(px as u64)) } else { None });

        let (on_done, on_done_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Set(req, on_done),
        }).await.map_err(|_| KvSetHandlerError::RuntimeShutdown)?;

        on_done_rx.await.map_err(|_| KvSetHandlerError::RuntimeShutdown)?.map_err(KvSetHandlerError::from)
    }

    let bytes: Bytes = req.into_body().collect().await.unwrap().to_bytes();
    let kv_set_response = handler(kv_tx, &bindings, bytes.as_ref()).await;

    let mut message = capnp::message::Builder::new_default();
    let response = message.init_root::<abi_kv_capnp::kv_set_response::Builder>();
    let mut response = response.init_response();

    match kv_set_response {
        Ok(()) => response.set_ok(()),
        Err(KvSetHandlerError::AlreadyExists) => response.set_already_exists(()),
        Err(KvSetHandlerError::RuntimeShutdown) => response.set_runtime_shutdown(()),
        Err(KvSetHandlerError::BindingNotFound) => response.set_binding_not_found(()),
        Err(KvSetHandlerError::BadRequest) => response.set_bad_request(()),
    }

    http::Response::new(HttpBody::for_bytes(capnp::serialize::write_message_to_words(&message).into()))
}

async fn handle_kv_publish(kv_tx: flume::Sender<KvMessage>, bindings: HashMap<String, KvBindingConfig>, req: http::Request<HttpBody>) -> http::Response<HttpBody> {
    async fn handler(kv_tx: flume::Sender<KvMessage>, bindings: &HashMap<String, KvBindingConfig>, mut request: &[u8]) -> Result<(), KvPublishHandlerError> {
        let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
        let request = request_reader.get_root::<abi_kv_capnp::kv_publish_request::Reader>().unwrap();

        let binding = request.get_binding().map_err(|_| KvPublishHandlerError::BadRequest)?;
        let binding = str::from_utf8(&binding.as_bytes()).map_err(|_| KvPublishHandlerError::BadRequest)?;
        let namespace = bindings.get(binding).ok_or(KvPublishHandlerError::BindingNotFound)?.namespace.clone();

        let channel = request.get_channel().map_err(|_| KvPublishHandlerError::BadRequest)?.to_vec();
        let data = request.get_data().map_err(|_| KvPublishHandlerError::BadRequest)?.to_vec();

        let (result_tx, result_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Publish(KvPublishRequest { channel, data }, result_tx),
        }).await.map_err(|_| KvPublishHandlerError::RuntimeShutdown)?;

        result_rx.await.map_err(|_| KvPublishHandlerError::RuntimeShutdown)
    }

    let bytes: Bytes = req.into_body().collect().await.unwrap().to_bytes();
    let kv_publish_response = handler(kv_tx, &bindings, bytes.as_ref()).await;

    let mut message = capnp::message::Builder::new_default();
    let response = message.init_root::<abi_kv_capnp::kv_publish_result::Builder>();
    let mut response = response.init_result();

    match kv_publish_response {
        Ok(()) => response.set_ok(()),
        Err(KvPublishHandlerError::RuntimeShutdown) => response.set_runtime_shutdown(()),
        Err(KvPublishHandlerError::BindingNotFound) => response.set_binding_not_found(()),
        Err(KvPublishHandlerError::BadRequest) => response.set_bad_request(()),
    }

    http::Response::new(HttpBody::for_bytes(capnp::serialize::write_message_to_words(&message).into()))
}

async fn handle_kv_delex_ifeq(kv_tx: flume::Sender<KvMessage>, bindings: HashMap<String, KvBindingConfig>, req: http::Request<HttpBody>) -> http::Response<HttpBody> {
    async fn handler(kv_tx: flume::Sender<KvMessage>, bindings: &HashMap<String, KvBindingConfig>, mut request: &[u8]) -> Result<(), KvDelexHandlerError> {
        let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
        let request = request_reader.get_root::<abi_kv_capnp::kv_delex_request::Reader>().unwrap();

        let binding = request.get_binding().map_err(|_| KvDelexHandlerError::BadRequest)?;
        let binding = str::from_utf8(&binding.as_bytes()).map_err(|_| KvDelexHandlerError::BadRequest)?;
        let namespace = bindings.get(binding).ok_or(KvDelexHandlerError::BindingNotFound)?.namespace.clone();

        let key = request.get_key().map_err(|_| KvDelexHandlerError::BadRequest)?.to_vec();
        let ifeq = request.get_ifeq().map_err(|_| KvDelexHandlerError::BadRequest)?.to_vec();

        let (result_tx, result_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Delex(KvDelexRequest { key, ifeq }, result_tx),
        }).await.map_err(|_| KvDelexHandlerError::RuntimeShutdown)?;

        result_rx.await.map_err(|_| KvDelexHandlerError::RuntimeShutdown)
    }

    let bytes: Bytes = req.into_body().collect().await.unwrap().to_bytes();
    let kv_delex_response = handler(kv_tx, &bindings, bytes.as_ref()).await;

    let mut message = capnp::message::Builder::new_default();
    let response = message.init_root::<abi_kv_capnp::kv_delex_result::Builder>();
    let mut response = response.init_result();

    match kv_delex_response {
        Ok(()) => response.set_ok(()),
        Err(KvDelexHandlerError::RuntimeShutdown) => response.set_runtime_shutdown(()),
        Err(KvDelexHandlerError::BindingNotFound) => response.set_binding_not_found(()),
        Err(KvDelexHandlerError::BadRequest) => response.set_bad_request(()),
    }

    http::Response::new(HttpBody::for_bytes(capnp::serialize::write_message_to_words(&message).into()))
}

async fn handle_kv_subscribe(kv_tx: flume::Sender<KvMessage>, bindings: HashMap<String, KvBindingConfig>, req: http::Request<HttpBody>) -> http::Response<HttpBody> {
    async fn handler(kv_tx: flume::Sender<KvMessage>, bindings: &HashMap<String, KvBindingConfig>, mut request: &[u8]) -> Result<flume::Receiver<Vec<u8>>, KvSubscriptionHandlerError> {
        let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
        let request = request_reader.get_root::<abi_kv_capnp::kv_subscribe_request::Reader>().unwrap();

        let binding = request.get_binding().map_err(|_| KvSubscriptionHandlerError::BadRequest)?;
        let binding = str::from_utf8(&binding.as_bytes()).map_err(|_| KvSubscriptionHandlerError::BadRequest)?;
        let namespace = bindings.get(binding).ok_or(KvSubscriptionHandlerError::BindingNotFound)?.namespace.clone();

        let channel = request.get_channel().map_err(|_| KvSubscriptionHandlerError::BadRequest)?.to_vec();

        let (result_tx, result_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Subscribe { channel, result: result_tx },
        }).await.map_err(|_| KvSubscriptionHandlerError::RuntimeShutdown)?;

        result_rx.await.map_err(|_| KvSubscriptionHandlerError::RuntimeShutdown)
    }

    let bytes: Bytes = req.into_body().collect().await.unwrap().to_bytes();
    let subscription = handler(kv_tx, &bindings, bytes.as_ref()).await;

    match subscription {
        Ok(receiver) => http::Response::new(HttpBody::for_stream(
            receiver.into_stream()
                .map(|v| Ok::<_, HttpStreamError>(Bytes::from(v)))
                .boxed()
        )),
        Err(err) => {
            let mut message = capnp::message::Builder::new_default();
            let response = message.init_root::<abi_kv_capnp::kv_subscribe_error::Builder>();
            let mut response = response.init_error();

            match &err {
                KvSubscriptionHandlerError::RuntimeShutdown => response.set_runtime_shutdown(()),
                KvSubscriptionHandlerError::BindingNotFound => response.set_binding_not_found(()),
                KvSubscriptionHandlerError::BadRequest => response.set_bad_request(()),
            }

            let mut response = http::Response::new(HttpBody::for_bytes(capnp::serialize::write_message_to_words(&message).into()));
            *response.status_mut() = match err {
                KvSubscriptionHandlerError::RuntimeShutdown => http::StatusCode::SERVICE_UNAVAILABLE,
                KvSubscriptionHandlerError::BindingNotFound => http::StatusCode::NOT_FOUND,
                KvSubscriptionHandlerError::BadRequest => http::StatusCode::BAD_REQUEST,
            };
            response
        },
    }
}
