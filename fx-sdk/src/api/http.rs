use {
    http::Uri,
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
        unimplemented!()
    }
}
