use crate::{
    function::abi::{capnp, abi_http_capnp},
    resources::serialize::SerializeResource,
};

pub(crate) struct FetchResult {
    status: ::http::StatusCode,
    body: Vec<u8>,
}

impl FetchResult {
    pub fn new(status: ::http::StatusCode, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
        }
    }
}

impl SerializeResource for FetchResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut fetch_response = message.init_root::<abi_http_capnp::http_response::Builder>();

        fetch_response.set_status(self.status.as_u16());
        fetch_response.set_body(&self.body);

        capnp::serialize::write_message_to_words(&message)
    }
}
