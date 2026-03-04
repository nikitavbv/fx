use crate::{
    function::abi::{capnp, abi_http_capnp},
    resources::{serialize::{SerializeResource, SerializableResource, DeserializeFunctionResource}, ResourceId},
    triggers::http::HttpBody,
};

pub(crate) enum FetchResult {
    Inline(FetchResultInline),
    BodyResource(SerializableResource<FetchResultWithBodyResource>),
}

impl FetchResult {
    pub fn new(parts: http::response::Parts, body: HttpBody) -> Self {
        Self::Inline(FetchResultInline::new(parts, body))
    }
}

pub(crate) struct FetchResultInline {
    parts: http::response::Parts,
    body: HttpBody,
}

impl FetchResultInline {
    pub fn new(parts: http::response::Parts, body: HttpBody) -> Self {
        Self {
            parts,
            body,
        }
    }

    pub fn into_parts(self) -> (http::response::Parts, HttpBody) {
        (self.parts, self.body)
    }
}

pub(crate) struct FetchResultWithBodyResource {
    parts: http::response::Parts,
    body: ResourceId,
}

impl FetchResultWithBodyResource {
    pub fn new(parts: http::response::Parts, body: ResourceId) -> Self {
        Self { parts, body }
    }
}

impl SerializeResource for FetchResultWithBodyResource {
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

        fetch_response.set_body_resource_id(self.body.as_u64());

        capnp::serialize::write_message_to_words(&message)
    }
}

pub(crate) enum HttpStreamFrame {}

impl DeserializeFunctionResource for HttpStreamFrame {
    fn deserialize(resource: &mut &[u8], instance: std::rc::Rc<crate::function::instance::FunctionInstance>) -> Self {
        todo!()
    }
}
