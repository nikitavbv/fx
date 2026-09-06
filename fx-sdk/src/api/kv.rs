use {
    std::{time::Duration, task::Poll},
    thiserror::Error,
    futures::{Stream, StreamExt},
    fx_types::{
        capnp,
        abi_kv_capnp,
    },
    crate::api::http::{FetchError, HttpBody},
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
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_kv_capnp::kv_delex_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_key(&key.into_bytes());
            message_request.set_ifeq(&ifeq.into_bytes());

            capnp::serialize::write_message_to_words(&message)
        };

        let result_vec = crate::api::http::fetch(
            crate::HttpRequest::post("http://kv.fx.internal/delex_ifeq").unwrap()
                .with_body(request)
        ).await.unwrap().bytes().await;

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();

        let resource = resource_reader.get_root::<abi_kv_capnp::kv_delex_result::Reader>().unwrap();
        let result = match resource.get_result().which().unwrap() {
            abi_kv_capnp::kv_delex_result::result::Which::Ok(_) => Ok(()),
            abi_kv_capnp::kv_delex_result::result::Which::BadRequest(())
            | abi_kv_capnp::kv_delex_result::result::Which::FailedToReadRequest(())
            | abi_kv_capnp::kv_delex_result::result::Which::ResourceNotFound(()) => Err(KvDelexError::InternalSdkError),
            abi_kv_capnp::kv_delex_result::result::Which::RuntimeShutdown(()) => Err(KvDelexError::RuntimeShutdown),
            abi_kv_capnp::kv_delex_result::result::Which::BindingNotFound(()) => Err(KvDelexError::BindingNotFound),
        };

        result.unwrap()
    }

    pub async fn subscribe(&self, channel: impl AsKey) -> Result<KvSubscriptionStream, KvSubscriptionStreamError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_kv_capnp::kv_subscribe_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_channel(&channel.into_bytes());

            capnp::serialize::write_message_to_words(&message)
        };

        let response = crate::api::http::fetch(
            crate::HttpRequest::post("http://kv.fx.internal/subscribe").unwrap()
                .with_body(request)
        ).await.map_err(|err| match err {
            FetchError::RuntimeShutdown => KvSubscriptionStreamError::RuntimeShutdown,
            _other => KvSubscriptionStreamError::InternalSdkError,
        })?;

        if response.status() != &http::StatusCode::OK {
            let error_bytes = response.bytes().await;
            let error_reader = capnp::serialize::read_message_from_flat_slice(&mut error_bytes.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
            let error = error_reader.get_root::<abi_kv_capnp::kv_subscribe_error::Reader>().unwrap();
            return Err(match error.get_error().which().unwrap() {
                abi_kv_capnp::kv_subscribe_error::error::Which::RuntimeShutdown(()) => KvSubscriptionStreamError::RuntimeShutdown,
                abi_kv_capnp::kv_subscribe_error::error::Which::BindingNotFound(()) => KvSubscriptionStreamError::BindingNotFound,
                abi_kv_capnp::kv_subscribe_error::error::Which::BadRequest(()) => KvSubscriptionStreamError::InternalSdkError,
            });
        }

        Ok(KvSubscriptionStream::new(response.into_body()))
    }

    pub async fn publish(&self, channel: impl AsKey, data: impl AsValue) -> Result<(), KvPublishError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_kv_capnp::kv_publish_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_channel(&channel.into_bytes());
            message_request.set_data(&data.into_bytes());

            capnp::serialize::write_message_to_words(&message)
        };

        let result_vec = crate::api::http::fetch(
            crate::HttpRequest::post("http://kv.fx.internal/publish").unwrap()
                .with_body(request)
        ).await.unwrap().bytes().await;

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();

        let resource = resource_reader.get_root::<abi_kv_capnp::kv_publish_result::Reader>().unwrap();
        match resource.get_result().which().unwrap() {
            abi_kv_capnp::kv_publish_result::result::Which::Ok(()) => Ok(()),
            abi_kv_capnp::kv_publish_result::result::Which::RuntimeShutdown(()) => Err(KvPublishError::RuntimeShutdown),
            abi_kv_capnp::kv_publish_result::result::Which::BindingNotFound(()) => Err(KvPublishError::BindingNotFound),
            abi_kv_capnp::kv_publish_result::result::Which::BadRequest(())
            | abi_kv_capnp::kv_publish_result::result::Which::FailedToReadRequest(()) => Err(KvPublishError::InternalSdkError),
        }
    }
}

pub struct KvSubscriptionStream(HttpBody);

impl KvSubscriptionStream {
    pub(crate) fn new(body: HttpBody) -> Self {
        Self(body)
    }
}

impl Stream for KvSubscriptionStream {
    type Item = Result<Vec<u8>, KvSubscriptionStreamError>;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().0.poll_next_unpin(cx)
            .map(|v| v.map(|frame| frame
                .map(|bytes| bytes.to_vec())
                .map_err(|_| KvSubscriptionStreamError::InternalSdkError)
            ))
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

#[derive(Debug, Error)]
pub enum KvPublishError {
    #[error("internal sdk error")]
    InternalSdkError,
    #[error("runtime is being shut down")]
    RuntimeShutdown,
    #[error("binding not found")]
    BindingNotFound,
}
