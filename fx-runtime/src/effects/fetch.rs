use crate::{
    function::abi::{capnp, abi_http_capnp},
    resources::serialize::SerializeResource,
};

pub(crate) struct FetchResult {
    parts: ::http::response::Parts,
    body: Vec<u8>,
}

impl FetchResult {
    pub fn new(parts: ::http::response::Parts, body: Vec<u8>) -> Self {
        Self {
            parts,
            body,
        }
    }
}

impl SerializeResource for FetchResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut fetch_response = message.init_root::<abi_http_capnp::http_response::Builder>();

        fetch_response.set_status(self.parts.status.as_u16());

        let mut headers = fetch_response.reborrow().init_headers(self.parts.headers.len() as u32);
        for (index, (name, value)) in self.parts.headers.iter().enumerate() {
            let mut header = headers.reborrow().get(index as u32);
            header.set_name(name.as_str());
            header.set_value(value.to_str().unwrap());
        }

        fetch_response.set_body(&self.body);

        capnp::serialize::write_message_to_words(&message)
    }
}
