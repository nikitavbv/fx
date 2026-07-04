use {
    std::time::Duration,
    thiserror::Error,
    futures::Stream,
    fx_types::{
        abi::{
            KvGetResponseFuturePollResult,
            KvGetResponseSerializeResult,
            KvSetResponseFuturePollResult,
            KvSetResponseSerializeResult,
            AsyncStreamResourcePollResult,
        },
        capnp,
        abi_kv_capnp,
    },
    crate::sys::{
        DeserializeHostResource,
        HostUnitFuture,
        fx_bytes_len,
        fx_bytes_move,
        fx_kv_set,
        fx_kv_set_nx_px,
        fx_kv_get,
        fx_kv_delex_ifeq,
        fx_kv_subscribe,
        fx_kv_publish,
        fx_kv_get_response_future_poll,
        fx_kv_get_response_serialize,
        fx_kv_set_response_future_poll,
        fx_kv_set_response_serialize,
        fx_kv_subscription_stream_poll_next,
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
        let (key_ptr, key_len) = key.as_key();
        let (value_ptr, value_len) = value.as_value();

        KvSetResponseFuture::new(unsafe { fx_kv_set(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            value_ptr,
            value_len,
        )}.into()).await.map(|_| ())
    }

    pub async fn set_nx_px(&self, key: impl AsKey, value: impl AsValue, nx: bool, px: Option<Duration>) -> Result<(), KvSetNxPxError> {
        let (key_ptr, key_len) = key.as_key();
        let (value_ptr, value_len) = value.as_value();

        KvSetResponseFuture::new(unsafe { fx_kv_set_nx_px(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            value_ptr,
            value_len,
            if nx { 1 } else { 0 },
            px.map(|v| v.as_millis() as i64).unwrap_or(-1)
        )}.into()).await.map(|v| match v {
            KvSetResponse::Ok => Ok(()),
            KvSetResponse::AlreadyExists => Err(KvSetNxPxError::AlreadyExists),
        })?.map_err(KvSetNxPxError::from)
    }

    pub async fn get(&self, key: impl AsKey) -> Result<Option<Vec<u8>>, KvGetError> {
    let (key_ptr, key_len) = key.as_key();

        KvGetResponseFuture::new(unsafe { fx_kv_get(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
        )}.into()).await.map(|v| match v {
            KvGetResponse::KeyNotFound => None,
            KvGetResponse::Some(v) => Some(v)
        })
    }

    pub async fn delex_ifeq(&self, key: impl AsKey, ifeq: impl AsValue) {
        let (key_ptr, key_len) = key.as_key();
        let (ifeq_ptr, ifeq_len) = ifeq.as_value();

        HostUnitFuture::new(unsafe { fx_kv_delex_ifeq(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            ifeq_ptr,
            ifeq_len,
        ) }).await
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

    pub async fn publish(&self, channel: impl AsKey, data: impl AsValue) {
        let (channel_ptr, channel_len) = channel.as_key();
        let (data_ptr, data_len) = data.as_value();

        HostUnitFuture::new(unsafe { fx_kv_publish(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            channel_ptr,
            channel_len,
            data_ptr,
            data_len
        ) }).await
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
        let mut result = std::mem::MaybeUninit::<AsyncStreamResourcePollResult>::zeroed();
        assert!(unsafe { fx_kv_subscription_stream_poll_next(self.resource_id, result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        match result.tag {
            2 => std::task::Poll::Pending,
            1 => std::task::Poll::Ready(Some({
                let bytes_len = unsafe { fx_bytes_len(result.resolved_resource_id) };

                let mut result_vec = vec![0; bytes_len as usize];
                unsafe { fx_bytes_move(result.resolved_resource_id, result_vec.as_mut_ptr() as u64) };
                Ok(result_vec)
            })),
            0 => std::task::Poll::Ready(None),
            _other => std::task::Poll::Ready(Some(Err(KvSubscriptionStreamError::InternalSdkError))),
        }
    }
}

#[derive(Debug, Error)]
pub enum KvSubscriptionStreamError {
    #[error("internal sdk error")]
    InternalSdkError,
}

// public api
pub trait AsKey {
    fn as_key(&self) -> (u64, u64);
}

impl AsKey for String {
    fn as_key(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }
}

impl AsKey for &str {
    fn as_key(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }
}

pub trait AsValue {
    fn as_value(&self) -> (u64, u64);
}

impl AsValue for String {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }
}

impl AsValue for &str {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }
}

impl AsValue for Vec<u8> {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }
}

impl AsValue for &Vec<u8> {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }
}

impl AsValue for &[u8] {
    fn as_value(&self) -> (u64, u64) {
        (self.as_ptr() as u64, self.len() as u64)
    }
}

#[derive(Debug, Error)]
pub enum KvSetNxPxError {
    #[error("nx condition violated: key already exists")]
    AlreadyExists,
    #[error("internal sdk error")]
    InternalSdkError,
}

impl From<KvSetError> for KvSetNxPxError {
    fn from(err: KvSetError) -> Self {
        match err {
            KvSetError::InternalSdkError => Self::InternalSdkError,
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

impl Into<u64> for &KvGetResponseResourceId {
    fn into(self) -> u64 {
        self.0
    }
}

struct KvGetResponseFuture(KvGetResponseResourceId);

impl KvGetResponseFuture {
    pub fn new(id: KvGetResponseResourceId) -> Self {
        Self(id)
    }
}

impl Future for KvGetResponseFuture {
    type Output = Result<KvGetResponse, KvGetError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<KvGetResponseFuturePollResult>::zeroed();
        assert!(unsafe { fx_kv_get_response_future_poll((&self.0).into(), result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<KvGetResponseSerializeResult>::zeroed();
                assert!(unsafe { fx_kv_get_response_serialize(result.kv_get_response_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                let resource = resource_reader.get_root::<abi_kv_capnp::kv_get_response::Reader>().unwrap();
                Ok(match resource.get_response().which().unwrap() {
                    abi_kv_capnp::kv_get_response::response::Which::KeyNotFound(_) => KvGetResponse::KeyNotFound,
                    abi_kv_capnp::kv_get_response::response::Which::Value(v) => KvGetResponse::Some(v.unwrap().to_vec()),
                })
            }),
            _other => std::task::Poll::Ready(Err(KvGetError::InternalSdkError)),
        }
    }
}

#[derive(Debug, Error)]
pub enum KvGetError {
    #[error("internal sdk error")]
    InternalSdkError,
}

struct KvSetResponseResourceId(u64);

impl From<u64> for KvSetResponseResourceId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl Into<u64> for &KvSetResponseResourceId {
    fn into(self) -> u64 {
        self.0
    }
}

struct KvSetResponseFuture(KvSetResponseResourceId);

impl KvSetResponseFuture {
    pub fn new(id: KvSetResponseResourceId) -> Self {
        Self(id)
    }
}

impl Future for KvSetResponseFuture {
    type Output = Result<KvSetResponse, KvSetError>;

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
                Ok(match resource.get_response().which().unwrap() {
                    abi_kv_capnp::kv_set_response::response::Which::Ok(_) => KvSetResponse::Ok,
                    abi_kv_capnp::kv_set_response::response::Which::AlreadyExists(_) => KvSetResponse::AlreadyExists,
                })
            }),
            _other => std::task::Poll::Ready(Err(KvSetError::InternalSdkError)),
        }
    }
}

#[derive(Debug, Error)]
pub enum KvSetError {
    #[error("internal sdk error")]
    InternalSdkError,
}

enum KvSetResponse {
    Ok,
    AlreadyExists,
}

enum KvGetResponse {
    KeyNotFound,
    Some(Vec<u8>),
}

impl DeserializeHostResource for KvGetResponse {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let resource = resource_reader.get_root::<abi_kv_capnp::kv_get_response::Reader>().unwrap();
        match resource.get_response().which().unwrap() {
            abi_kv_capnp::kv_get_response::response::Which::KeyNotFound(_) => KvGetResponse::KeyNotFound,
            abi_kv_capnp::kv_get_response::response::Which::Value(v) => KvGetResponse::Some(v.unwrap().to_vec()),
        }
    }
}
