use crate::{
    function::abi::{capnp, abi_blob_capnp},
    resources::serialize::SerializeResource,
};

pub(crate) enum BlobGetResponse {
    NotFound,
    Ok(Vec<u8>),
    BindingNotExists,
}

impl SerializeResource for BlobGetResponse {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let blob_get_response = message.init_root::<abi_blob_capnp::blob_get_response::Builder>();
        let mut response = blob_get_response.init_response();

        match self {
            Self::NotFound => response.set_not_found(()),
            Self::Ok(v) => response.set_value(&v),
            Self::BindingNotExists => response.set_binding_not_exists(()),
        }

        capnp::serialize::write_message_to_words(&message)
    }
}
