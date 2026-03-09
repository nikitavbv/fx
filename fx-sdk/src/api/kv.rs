use {
    std::time::Duration,
    fx_types::{abi_kv_capnp, capnp},
    thiserror::Error,
    crate::sys::{
        DeserializeHostResource,
        FutureHostResource,
        HostUnitFuture,
        OwnedResourceId,
        fx_kv_set,
        fx_kv_set_nx_px,
        fx_kv_get,
        fx_kv_delex_ifeq,
    },
};

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

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_kv_set(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            value_ptr,
            value_len,
        ) });

        HostUnitFuture::new(resource_id).await;
    }

    pub async fn set_nx_px(&self, key: impl AsKey, value: impl AsValue, nx: bool, px: Option<Duration>) -> Result<(), KvSetNxPxError> {
        let (key_ptr, key_len) = key.as_key();
        let (value_ptr, value_len) = value.as_value();

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_kv_set_nx_px(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            value_ptr,
            value_len,
            if nx { 1 } else { 0 },
            px.map(|v| v.as_millis() as i64).unwrap_or(-1)
        ) });

        match FutureHostResource::<KvSetResponse>::new(resource_id).await {
            KvSetResponse::Ok => Ok(()),
            KvSetResponse::AlreadyExists => Err(KvSetNxPxError::AlreadyExists),
        }
    }

    pub async fn get(&self, key: impl AsKey) -> Option<Vec<u8>> {
        let (key_ptr, key_len) = key.as_key();

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_kv_get(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
        ) });

        match FutureHostResource::<KvGetResponse>::new(resource_id).await {
            KvGetResponse::KeyNotFound => None,
            KvGetResponse::Some(v) => Some(v),
        }
    }

    pub async fn delex_ifeq(&self, key: impl AsKey, ifeq: impl AsValue) {
        let (key_ptr, key_len) = key.as_key();
        let (ifeq_ptr, ifeq_len) = ifeq.as_value();

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_kv_delex_ifeq(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key_ptr,
            key_len,
            ifeq_ptr,
            ifeq_len,
        ) });

        HostUnitFuture::new(resource_id).await
    }
}

// public api
trait AsKey {
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

trait AsValue {
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
enum KvSetResponse {
    Ok,
    AlreadyExists,
}

impl DeserializeHostResource for KvSetResponse {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let resource = resource_reader.get_root::<abi_kv_capnp::kv_set_response::Reader>().unwrap();
        match resource.get_response().which().unwrap() {
            abi_kv_capnp::kv_set_response::response::Which::Ok(_) => KvSetResponse::Ok,
            abi_kv_capnp::kv_set_response::response::Which::AlreadyExists(_) => KvSetResponse::AlreadyExists,
        }
    }
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
