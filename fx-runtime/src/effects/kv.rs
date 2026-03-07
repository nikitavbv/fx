use {
    fx_types::{capnp, abi_kv_capnp},
    crate::resources::serialize::SerializeResource,
};

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
