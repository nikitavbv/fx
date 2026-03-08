use {
    std::time::Duration,
    thiserror::Error,
    fx_types::{capnp, abi_kv_capnp},
    crate::resources::serialize::SerializeResource,
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

impl SerializeResource for KvGetResponse {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let kv_get_response = message.init_root::<abi_kv_capnp::kv_get_response::Builder>();
        let mut response = kv_get_response.init_response();

        match self {
            Self::KeyNotFound => response.set_key_not_found(()),
            Self::Ok(v) => response.set_value(&v),
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

impl SerializeResource for Result<(), KvSetError> {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let kv_set_response = message.init_root::<abi_kv_capnp::kv_set_response::Builder>();
        let mut response = kv_set_response.init_response();

        match self {
            Self::Ok(v) => response.set_ok(v),
            Self::Err(KvSetError::AlreadyExists) => response.set_already_exists(()),
        }

        capnp::serialize::write_message_segments_to_words(&message)
    }
}
