use crate::sys::ResourceId;

pub struct HttpRequest(HttpRequestInner);

impl HttpRequest {
    pub fn from_host_resource(resource: ResourceId) -> Self {
        Self(HttpRequestInner::Host(resource))
    }
}

enum HttpRequestInner {
    Host(ResourceId),
}
