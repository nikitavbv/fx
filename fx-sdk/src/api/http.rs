use {
    std::str::FromStr,
    http::Uri,
    fx_types::{capnp, abi_host_resources_capnp},
    crate::sys::{ResourceId, DeserializableHostResource, DeserializeHostResource},
};

pub struct HttpRequest(DeserializableHostResource<HttpRequestInner>);

impl HttpRequest {
    pub fn from_host_resource(resource: ResourceId) -> Self {
        Self(DeserializableHostResource::from(resource))
    }

    pub fn uri(&self) -> &Uri {
        &self.0.get_raw().url
    }
}

struct HttpRequestInner {
    url: Uri,
}

impl DeserializeHostResource for HttpRequestInner {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<fx_types::abi_host_resources_capnp::function_request::Reader>().unwrap();

        HttpRequestInner {
            url: Uri::from_str(request.get_uri().unwrap().to_str().unwrap()).unwrap(),
        }
    }
}
