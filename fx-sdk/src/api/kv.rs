use {
    std::{time::Duration, task::Poll},
    thiserror::Error,
    futures::Stream,
    fx_types::{
        abi::{
            KvSetResponseFuturePollResult,
            KvSetResponseSerializeResult,
            KvSubscriptionStreamPollResult,
            AsyncResourcePollResult,
            ResourceSerializeResult,
        },
        capnp,
        abi_kv_capnp,
    },
    crate::sys::{
        fx_bytes_len,
        fx_bytes_move,
        fx_kv_set,
        fx_kv_set_nx_px,
        fx_kv_delex_ifeq,
        fx_kv_subscribe,
        fx_kv_publish,
        fx_kv_set_response_future_poll,
        fx_kv_set_response_serialize,
        fx_kv_subscription_stream_poll_next,
        fx_kv_publish_result_future_poll,
        fx_kv_publish_result_serialize,
        fx_kv_delex_result_future_poll,
        fx_kv_delex_result_serialize,
    },
};

#[derive(Clone, Debug)]
pub struct Kv {
    binding: String,
}

impl Kv {
    pub fn new(binding: impl Into<String>) -> Self {
        Self {
            binding: binding.into(),
        }
    }

    pub async fn set(&self, key: impl AsKey, value: impl AsValue) -> Result<(), KvSetError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_kv_capnp::kv_set_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_key(&key.into_bytes());
            message_request.set_value(&value.into_bytes());

            capnp::serialize::write_message_to_words(&message)
        };

        let result_vec = crate::api::http::fetch(
            crate::HttpRequest::post("http://kv.fx.internal/set").unwrap()
                .with_body(request)
        ).await.unwrap().bytes().await;

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();

        let resource = resource_reader.get_root::<abi_kv_capnp::kv_set_response::Reader>().unwrap();
        match resource.get_response().which().unwrap() {
            abi_kv_capnp::kv_set_response::response::Which::Ok(()) => Ok(()),
            abi_kv_capnp::kv_set_response::response::Which::AlreadyExists(_) => Err(KvSetError::AlreadyExists),
            abi_kv_capnp::kv_set_response::response::Which::RuntimeShutdown(()) => Err(KvSetError::RuntimeShutdown),
            abi_kv_capnp::kv_set_response::response::Which::BindingNotFound(()) => Err(KvSetError::BindingNotFound),
            abi_kv_capnp::kv_set_response::response::Which::FailedToReadRequest(())
            | abi_kv_capnp::kv_set_response::response::Which::BadRequest(()) => Err(KvSetError::InternalSdkError),
        }
    }

    pub async fn set_nx_px(&self, key: impl AsKey, value: impl AsValue, nx: bool, px: Option<Duration>) -> Result<(), KvSetNxPxError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_kv_capnp::kv_set_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_key(&key.into_bytes());
            message_request.set_value(&value.into_bytes());
            message_request.set_nx(if nx { 1 } else { 0 });
            if let Some(px) = px {
                message_request.set_px(px.as_millis() as i64);
            }

            capnp::serialize::write_message_to_words(&message)
        };

        let result_vec = crate::api::http::fetch(
            crate::HttpRequest::post("http://kv.fx.internal/set").unwrap()
                .with_body(request)
        ).await.unwrap().bytes().await;

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();

        let resource = resource_reader.get_root::<abi_kv_capnp::kv_set_response::Reader>().unwrap();
        match resource.get_response().which().unwrap() {
            abi_kv_capnp::kv_set_response::response::Which::Ok(()) => Ok(()),
            abi_kv_capnp::kv_set_response::response::Which::AlreadyExists(_) => Err(KvSetNxPxError::AlreadyExists),
            abi_kv_capnp::kv_set_response::response::Which::RuntimeShutdown(()) => Err(KvSetNxPxError::RuntimeShutdown),
            abi_kv_capnp::kv_set_response::response::Which::BindingNotFound(()) => Err(KvSetNxPxError::BindingNotFound),
            abi_kv_capnp::kv_set_response::response::Which::FailedToReadRequest(())
            | abi_kv_capnp::kv_set_response::response::Which::BadRequest(()) => Err(KvSetNxPxError::InternalSdkError),
        }
    }

    pub async fn get(&self, key: impl AsKey) -> Result<Option<Vec<u8>>, KvGetError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_kv_capnp::kv_get_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_key(&key.into_bytes());

            capnp::serialize::write_message_to_words(&message)
        };

        let result_vec = crate::api::http::fetch(
            crate::HttpRequest::post("http://kv.fx.internal/get").unwrap()
                .with_body(request)
        ).await.unwrap().bytes().await;

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
        let resource = resource_reader.get_root::<abi_kv_capnp::kv_get_response::Reader>().unwrap();
        match resource.get_response().which().unwrap() {
            abi_kv_capnp::kv_get_response::response::Which::KeyNotFound(()) => Ok(None),
            abi_kv_capnp::kv_get_response::response::Which::Value(v) => Ok(Some(v.unwrap().to_vec())),
            abi_kv_capnp::kv_get_response::response::Which::RuntimeShutdown(()) => Err(KvGetError::RuntimeShutdown),
            abi_kv_capnp::kv_get_response::response::Which::BindingNotFound(()) => Err(KvGetError::BindingNotFound),
            abi_kv_capnp::kv_get_response::response::Which::BadRequest(())
            | abi_kv_capnp::kv_get_response::response::Which::FailedToReadRequest(()) => Err(KvGetError::InternalSdkError),
        }
    }

    pub async fn delex_ifeq(&self, key: impl AsKey, ifeq: impl AsValue) {
        let (key_ptr, key_len) = key.as_key();
        let (ifeq_ptr, ifeq_len) = ifeq.as_value();

        KvDelexResultFuture::new(unsafe { fx_kv_delex_ifeq(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            ifeq_ptr,
            ifeq_len,
        ) }.into()).await.unwrap()
    }

    pub async fn subscribe(&self, channel: impl AsKey) -> KvSubscriptionStream {
        let (channel_ptr, channel_len) = channel.as_key();

        KvSubscriptionStream::new(unsafe { fx_kv_subscribe(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            channel_ptr,
            channel_len,
        ) })
    }

    pub async fn publish(&self, channel: impl AsKey, data: impl AsValue) -> Result<(), KvPublishError> {
        let (channel_ptr, channel_len) = channel.as_key();
        let (data_ptr, data_len) = data.as_value();

        KvPublishResultFuture::new(unsafe { fx_kv_publish(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            channel_ptr,
            channel_len,
            data_ptr,
            data_len
        ) }.into()).await
    }
}

pub struct KvSubscriptionStream {
    resource_id: u64,
}

impl KvSubscriptionStream {
    pub fn new(resource_id: u64) -> Self {
        Self {
            resource_id,
        }
    }
}

impl Stream for KvSubscriptionStream {
    type Item = Result<Vec<u8>, KvSubscriptionStreamError>;

    fn poll_next(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        let mut result = std::mem::MaybeUninit::<KvSubscriptionStreamPollResult>::zeroed();
        assert!(unsafe { fx_kv_subscription_stream_poll_next(self.resource_id, result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        match result.tag {
            0 => Poll::Ready(None),
            1 => Poll::Ready(Some({
                let bytes_len = unsafe { fx_bytes_len(result.resolved_resource_id) };

                let mut result_vec = vec![0; bytes_len as usize];
                unsafe { fx_bytes_move(result.resolved_resource_id, result_vec.as_mut_ptr() as u64) };
                Ok(result_vec)
            })),
            2 => Poll::Pending,
            3 => Poll::Ready(Some(Err(KvSubscriptionStreamError::RuntimeShutdown))),
            4 => Poll::Ready(Some(Err(KvSubscriptionStreamError::BindingNotFound))),
            5 => Poll::Ready(Some(Err(KvSubscriptionStreamError::InternalSdkError))),
            _other => std::task::Poll::Ready(Some(Err(KvSubscriptionStreamError::InternalSdkError))),
        }
    }
}

#[derive(Debug, Error)]
pub enum KvSubscriptionStreamError {
    #[error("internal sdk error")]
    InternalSdkError,
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding with this name is not found")]
    BindingNotFound,
}

// public api
pub trait AsKey {
    fn as_key(&self) -> (u64, u64);
    fn into_bytes(self) -> Vec<u8>;
}

impl AsKey for String {
    fn as_key(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.into_bytes()
    }
}

impl AsKey for &str {
    fn as_key(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

pub trait AsValue {
    fn as_value(&self) -> (u64, u64);
    fn into_bytes(self) -> Vec<u8>;
}

impl AsValue for String {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.into_bytes()
    }
}

impl AsValue for &str {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl AsValue for Vec<u8> {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }

    fn into_bytes(self) -> Vec<u8> {
        self
    }
}

impl AsValue for &Vec<u8> {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.clone()
    }
}

impl AsValue for &[u8] {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.to_vec()
    }
}

#[derive(Debug, Error)]
pub enum KvSetNxPxError {
    #[error("nx condition violated: key already exists")]
    AlreadyExists,
    #[error("internal sdk error")]
    InternalSdkError,
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding not found")]
    BindingNotFound,
}

impl From<KvSetError> for KvSetNxPxError {
    fn from(err: KvSetError) -> Self {
        match err {
            KvSetError::InternalSdkError => Self::InternalSdkError,
            KvSetError::RuntimeShutdown => Self::RuntimeShutdown,
            KvSetError::AlreadyExists => Self::AlreadyExists,
            KvSetError::BindingNotFound => Self::BindingNotFound,
        }
    }
}

// abi
struct KvGetResponseResourceId(u64);

impl From<u64> for KvGetResponseResourceId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<&KvGetResponseResourceId> for u64 {
    fn from(id: &KvGetResponseResourceId) -> u64 {
        id.0
    }
}

#[derive(Debug, Error)]
pub enum KvGetError {
    #[error("internal sdk error")]
    InternalSdkError,
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("kv binding with requested name is not found")]
    BindingNotFound,
}

struct KvSetResponseResourceId(u64);

impl From<u64> for KvSetResponseResourceId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<&KvSetResponseResourceId> for u64 {
    fn from(value: &KvSetResponseResourceId) -> u64 {
        value.0
    }
}

struct KvSetResponseFuture(KvSetResponseResourceId);

impl KvSetResponseFuture {
    pub fn new(id: KvSetResponseResourceId) -> Self {
        Self(id)
    }
}

impl Future for KvSetResponseFuture {
    type Output = Result<(), KvSetError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<KvSetResponseFuturePollResult>::zeroed();
        assert!(unsafe { fx_kv_set_response_future_poll((&self.0).into(), result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<KvSetResponseSerializeResult>::zeroed();
                assert!(unsafe { fx_kv_set_response_serialize(result.kv_set_response_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();

                let resource = resource_reader.get_root::<abi_kv_capnp::kv_set_response::Reader>().unwrap();
                match resource.get_response().which().unwrap() {
                    abi_kv_capnp::kv_set_response::response::Which::Ok(()) => Ok(()),
                    abi_kv_capnp::kv_set_response::response::Which::AlreadyExists(_) => Err(KvSetError::AlreadyExists),
                    abi_kv_capnp::kv_set_response::response::Which::RuntimeShutdown(()) => Err(KvSetError::RuntimeShutdown),
                    abi_kv_capnp::kv_set_response::response::Which::BindingNotFound(()) => Err(KvSetError::BindingNotFound),
                    abi_kv_capnp::kv_set_response::response::Which::FailedToReadRequest(())
                    | abi_kv_capnp::kv_set_response::response::Which::BadRequest(()) => Err(KvSetError::InternalSdkError),
                }
            }),
            _other => std::task::Poll::Ready(Err(KvSetError::InternalSdkError)),
        }
    }
}

#[derive(Debug, Error)]
pub enum KvSetError {
    #[error("key already exists")]
    AlreadyExists,
    #[error("runtime is being shutdown")]
    RuntimeShutdown,
    #[error("internal sdk error")]
    InternalSdkError,
    #[error("kv binding with this name is not found")]
    BindingNotFound,
}

#[derive(Debug, Error)]
pub enum KvDelexError {
    #[error("internal sdk error")]
    InternalSdkError,
    #[error("request was not processed because runtime is being shut down")]
    RuntimeShutdown,
    #[error("kv binding with this name is not found")]
    BindingNotFound,
}

struct KvDelexResultResourceId(u64);

impl From<u64> for KvDelexResultResourceId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<&KvDelexResultResourceId> for u64 {
    fn from(value: &KvDelexResultResourceId) -> Self {
        value.0
    }
}

struct KvDelexResultFuture(KvDelexResultResourceId);

impl KvDelexResultFuture {
    pub fn new(id: KvDelexResultResourceId) -> Self {
        Self(id)
    }
}

impl Future for KvDelexResultFuture {
    type Output = Result<(), KvDelexError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<AsyncResourcePollResult>::zeroed();
        assert!(unsafe { fx_kv_delex_result_future_poll((&self.0).into(), result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<ResourceSerializeResult>::zeroed();
                assert!(unsafe { fx_kv_delex_result_serialize(result.resolved_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();

                let resource = resource_reader.get_root::<abi_kv_capnp::kv_delex_result::Reader>().unwrap();
                match resource.get_result().which().unwrap() {
                    abi_kv_capnp::kv_delex_result::result::Which::Ok(_) => Ok(()),
                    abi_kv_capnp::kv_delex_result::result::Which::BadRequest(())
                    | abi_kv_capnp::kv_delex_result::result::Which::FailedToReadRequest(())
                    | abi_kv_capnp::kv_delex_result::result::Which::ResourceNotFound(()) => Err(KvDelexError::InternalSdkError),
                    abi_kv_capnp::kv_delex_result::result::Which::RuntimeShutdown(()) => Err(KvDelexError::RuntimeShutdown),
                    abi_kv_capnp::kv_delex_result::result::Which::BindingNotFound(()) => Err(KvDelexError::BindingNotFound),
                }
            }),
            _other => std::task::Poll::Ready(Err(KvDelexError::InternalSdkError)),
        }
    }
}

struct KvPublishResultResourceId(u64);

impl From<u64> for KvPublishResultResourceId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<&KvPublishResultResourceId> for u64 {
    fn from(id: &KvPublishResultResourceId) -> Self {
        id.0
    }
}

#[derive(Debug, Error)]
pub enum KvPublishError {
    #[error("internal sdk error")]
    InternalSdkError,
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding not found")]
    BindingNotFound,
}

struct KvPublishResultFuture(KvPublishResultResourceId);

impl KvPublishResultFuture {
    pub fn new(id: KvPublishResultResourceId) -> Self {
        Self(id)
    }
}

impl Future for KvPublishResultFuture {
    type Output = Result<(), KvPublishError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<AsyncResourcePollResult>::zeroed();
        assert!(unsafe { fx_kv_publish_result_future_poll((&self.0).into(), result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<ResourceSerializeResult>::zeroed();
                assert!(unsafe { fx_kv_publish_result_serialize(result.resolved_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();

                let resource = resource_reader.get_root::<abi_kv_capnp::kv_publish_result::Reader>().unwrap();
                match resource.get_result().which().unwrap() {
                    abi_kv_capnp::kv_publish_result::result::Which::Ok(()) => Ok(()),
                    abi_kv_capnp::kv_publish_result::result::Which::RuntimeShutdown(()) => Err(KvPublishError::RuntimeShutdown),
                    abi_kv_capnp::kv_publish_result::result::Which::BindingNotFound(()) => Err(KvPublishError::BindingNotFound),
                    abi_kv_capnp::kv_publish_result::result::Which::BadRequest(())
                    | abi_kv_capnp::kv_publish_result::result::Which::FailedToReadRequest(()) => Err(KvPublishError::InternalSdkError),
                }
            }),
            _other => std::task::Poll::Ready(Err(KvPublishError::InternalSdkError)),
        }
    }
}
