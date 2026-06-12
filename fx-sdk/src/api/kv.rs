use {
    std::time::Duration,
    thiserror::Error,
    futures::Stream,
    fx_types::{
        abi::{
            FuturePollResult,
            KvGetResponseFuturePollResult,
            KvGetResponseSerializeResult,
            KvSetResponseFuturePollResult,
            KvSetResponseSerializeResult,
        },
        capnp,
        abi_kv_capnp,
    },
    crate::sys::{
        DeserializeHostResource,
        FutureHostResource,
        HostUnitFuture,
        OwnedResourceId,
        fx_kv_set,
        fx_kv_set_nx_px,
        fx_kv_get,
        fx_kv_delex_ifeq,
        fx_kv_subscribe,
        fx_kv_publish,
        fx_future_poll,
        fx_stream_frame_read,
        fx_resource_serialize,
        fx_kv_get_response_future_poll,
        fx_kv_get_response_serialize,
        fx_bytes_move,
        fx_kv_set_response_future_poll,
        fx_kv_set_response_serialize,
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

    pub async fn set(&self, key: impl AsKey, value: impl AsValue) {
        let (key_ptr, key_len) = key.as_key();
        let (value_ptr, value_len) = value.as_value();

        KvSetResponseFuture::new(unsafe { fx_kv_set(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            value_ptr,
            value_len,
        )}.into()).await;
    }

    pub async fn set_nx_px(&self, key: impl AsKey, value: impl AsValue, nx: bool, px: Option<Duration>) -> Result<(), KvSetNxPxError> {
        let (key_ptr, key_len) = key.as_key();
        let (value_ptr, value_len) = value.as_value();

        match KvSetResponseFuture::new(unsafe { fx_kv_set_nx_px(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            value_ptr,
            value_len,
            if nx { 1 } else { 0 },
            px.map(|v| v.as_millis() as i64).unwrap_or(-1)
        )}.into()).await {
            KvSetResponse::Ok => Ok(()),
            KvSetResponse::AlreadyExists => Err(KvSetNxPxError::AlreadyExists),
        }
    }

    pub async fn get(&self, key: impl AsKey) -> Option<Vec<u8>> {
        let (key_ptr, key_len) = key.as_key();

        match KvGetResponseFuture::new(unsafe { fx_kv_get(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
        )}.into()).await {
            KvGetResponse::KeyNotFound => None,
            KvGetResponse::Some(v) => Some(v),
        }
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

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_kv_subscribe(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            channel_ptr,
            channel_len,
        ) });

        KvSubscriptionStream::new(resource_id)
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
    resource_id: OwnedResourceId,
}

impl KvSubscriptionStream {
    pub fn new(resource_id: OwnedResourceId) -> Self {
        Self {
            resource_id,
        }
    }
}

impl Stream for KvSubscriptionStream {
    type Item = Vec<u8>;

    fn poll_next(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        let poll_result = self.resource_id.with(|resource_id| unsafe { fx_future_poll(resource_id.as_ffi()) });
        let poll_result = FuturePollResult::try_from(poll_result).unwrap();
        if let FuturePollResult::Pending = poll_result {
            return std::task::Poll::Pending;
        }

        let frame_length = self.resource_id.with(|resource_id| unsafe { fx_resource_serialize(resource_id.as_ffi()) });
        let data: Vec<u8> = vec![0u8; frame_length as usize];
        self.resource_id.with(|resource_id| unsafe { fx_stream_frame_read(resource_id.as_ffi(), data.as_ptr() as u64); });

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut data.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
        let frame = resource_reader.get_root::<abi_kv_capnp::kv_subscription_frame::Reader>().unwrap();
        std::task::Poll::Ready(match frame.get_frame().which().unwrap() {
            abi_kv_capnp::kv_subscription_frame::frame::Which::StreamEnd(_) => None,
            abi_kv_capnp::kv_subscription_frame::frame::Which::Bytes(v) => Some(v.unwrap().to_vec()),
        })
    }
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
    type Output = KvGetResponse;

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
                match resource.get_response().which().unwrap() {
                    abi_kv_capnp::kv_get_response::response::Which::KeyNotFound(_) => KvGetResponse::KeyNotFound,
                    abi_kv_capnp::kv_get_response::response::Which::Value(v) => KvGetResponse::Some(v.unwrap().to_vec()),
                }
            }),
            other => todo!(),
        }
    }
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
    type Output = KvSetResponse;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
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
                    abi_kv_capnp::kv_set_response::response::Which::Ok(_) => KvSetResponse::Ok,
                    abi_kv_capnp::kv_set_response::response::Which::AlreadyExists(_) => KvSetResponse::AlreadyExists,
                }
            }),
            other => todo!(),
        }
    }
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
