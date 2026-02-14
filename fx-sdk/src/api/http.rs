use {
    std::str::FromStr,
    http::{Uri, Method},
    fx_types::{capnp, abi_host_resources_capnp},
    crate::sys::{ResourceId, DeserializableHostResource, DeserializeHostResource},
};

pub struct HttpRequest(HttpRequestInner);

/// http request can be owned by host or function
enum HttpRequestInner {
    Host(DeserializableHostResource<HttpRequestData>),
    Function(HttpRequestData),
}

impl HttpRequest {
    pub fn new(method: Method, url: Uri) -> Self {
        Self(HttpRequestInner::Function(HttpRequestData::new(method, url)))
    }

    pub fn from_host_resource(resource: ResourceId) -> Self {
        Self(HttpRequestInner::Host(DeserializableHostResource::from(resource)))
    }

    fn request_data(&self) -> &HttpRequestData {
        match &self.0 {
            HttpRequestInner::Host(v) => v.get_raw(),
            HttpRequestInner::Function(v) => v,
        }
    }

    pub fn method(&self) -> &Method {
        &self.request_data().method
    }

    pub fn uri(&self) -> &Uri {
        &self.request_data().url
    }
}

struct HttpRequestData {
    method: Method,
    url: Uri,
}

impl HttpRequestData {
    pub fn new(method: Method, url: Uri) -> Self {
        Self { method, url }
    }
}

impl DeserializeHostResource for HttpRequestData {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<fx_types::abi_host_resources_capnp::function_request::Reader>().unwrap();

        HttpRequestData {
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

pub struct HttpResponse;

pub async fn fetch(request: HttpRequest) -> HttpResponse {
    todo!()
}
