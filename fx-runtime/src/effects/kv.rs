use {
    std::time::Duration,
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
