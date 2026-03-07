use {
    fx_types::{capnp, abi_kv_capnp},
    crate::sys::{fx_kv_set, fx_kv_get, OwnedResourceId, HostUnitFuture, FutureHostResource, DeserializeHostResource},
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

    pub async fn set(&self, key: impl Into<KvKey>, value: impl Into<KvValue>) {
        let key = key.into();
        let value = value.into();

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_kv_set(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.value.as_ptr() as u64,
            key.value.len() as u64,
            value.value.as_ptr() as u64,
            value.value.len() as u64,
        ) });

        HostUnitFuture::new(resource_id).await;
    }

    pub async fn get(&self, key: impl Into<KvValue>) -> Option<Vec<u8>> {
        let key = key.into();

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_kv_get(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.value.as_ptr() as u64,
            key.value.len() as u64,
        ) });

        match FutureHostResource::<KvGetResponse>::new(resource_id).await {
            KvGetResponse::KeyNotFound => None,
            KvGetResponse::Some(v) => Some(v),
        }
    }
}

struct KvKey {
    value: Vec<u8>,
}

impl From<String> for KvKey {
    fn from(value: String) -> Self {
        Self { value: value.into_bytes() }
    }
}

impl From<&str> for KvKey {
    fn from(value: &str) -> Self {
        Self { value: value.as_bytes().to_vec() }
    }
}

struct KvValue {
    value: Vec<u8>,
}

impl From<String> for KvValue {
    fn from(value: String) -> Self {
        Self { value: value.into_bytes() }
    }
}

impl From<&str> for KvValue {
    fn from(value: &str) -> Self {
        Self { value: value.as_bytes().to_vec() }
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
