use {
    std::str::FromStr,
    http::{Uri, Method},
    fx_types::{capnp, abi_host_resources_capnp},
    crate::sys::{ResourceId, DeserializableHostResource, DeserializeHostResource},
};

pub struct HttpRequest(DeserializableHostResource<HttpRequestInner>);

impl HttpRequest {
    pub fn from_host_resource(resource: ResourceId) -> Self {
        Self(DeserializableHostResource::from(resource))
    }

    pub fn method(&self) -> &Method {
        &self.0.get_raw().method
    }

    pub fn uri(&self) -> &Uri {
        &self.0.get_raw().url
    }
}

struct HttpRequestInner {
    method: Method,
    url: Uri,
}

impl DeserializeHostResource for HttpRequestInner {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<fx_types::abi_host_resources_capnp::function_request::Reader>().unwrap();

        HttpRequestInner {
            method: match &request.get_method().unwrap() {
                abi_host_resources_capnp::HttpMethod::Get => Method::GET,
                abi_host_resources_capnp::HttpMethod::Delete => Method::DELETE,
                abi_host_resources_capnp::HttpMethod::Options => Method::OPTIONS,
                abi_host_resources_capnp::HttpMethod::Patch => Method::PATCH,
                abi_host_resources_capnp::HttpMethod::Post => Method::POST,
                abi_host_resources_capnp::HttpMethod::Put => Method::PUT,
            },
            url: Uri::from_str(request.get_uri().unwrap().to_str().unwrap()).unwrap(),
        }
    }
}
