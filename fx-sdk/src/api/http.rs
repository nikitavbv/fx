pub use http::{HeaderName, HeaderValue};

use {
    std::str::FromStr,
    http::{Uri, Method, HeaderMap},
    thiserror::Error,
    fx_types::{capnp, abi_http_capnp},
    crate::sys::{
        ResourceId,
        DeserializableHostResource,
        DeserializeHostResource,
        SerializeResource,
        SerializableResource,
        OwnedResourceId,
        FutureHostResource,
        fx_fetch,
    },
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

    pub fn get(url: impl Into<String>) -> Result<Self, ()> {
        Ok(Self::new(Method::GET, url.into().parse().unwrap()))
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

    pub fn headers(&self) -> &HeaderMap {
        &self.request_data().headers
    }
}

pub(crate) struct HttpRequestData {
    method: Method,
    url: Uri,
    headers: HeaderMap,
}

impl HttpRequestData {
    pub fn new(method: Method, url: Uri) -> Self {
        Self {
            method,
            url,
            headers: HeaderMap::new(),
        }
    }
}

impl DeserializeHostResource for HttpRequestData {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<fx_types::abi_http_capnp::http_request::Reader>().unwrap();

        HttpRequestData {
            method: match &request.get_method().unwrap() {
                abi_http_capnp::HttpMethod::Get => Method::GET,
                abi_http_capnp::HttpMethod::Delete => Method::DELETE,
                abi_http_capnp::HttpMethod::Options => Method::OPTIONS,
                abi_http_capnp::HttpMethod::Patch => Method::PATCH,
                abi_http_capnp::HttpMethod::Post => Method::POST,
                abi_http_capnp::HttpMethod::Put => Method::PUT,
            },
            url: Uri::from_str(request.get_uri().unwrap().to_str().unwrap()).unwrap(),
            headers: request.get_headers().unwrap().into_iter()
                .map(|header| (
                    HeaderName::from_bytes(header.get_name().unwrap().as_bytes()).unwrap(),
                    HeaderValue::from_bytes(header.get_value().unwrap().as_bytes()).unwrap()
                ))
                .collect(),
        }
    }
}

pub struct HttpResponse {
    pub status: http::StatusCode,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new() -> Self {
        Self {
            status: http::StatusCode::OK,
            body: Vec::new(),
        }
    }

    pub fn with_status(mut self, status: http::StatusCode) -> Self {
        self.status = status;
        self
    }

    pub fn with_body(mut self, body: impl HttpResponseBody) -> Self {
        self.body = body.into_bytes();
        self
    }
}

pub async fn fetch(request: HttpRequest) -> Result<HttpResponse, FetchError> {
    let fetch = {
        let mut message = capnp::message::Builder::new_default();
        let mut fetch = message.init_root::<abi_http_capnp::http_request::Builder>();
        fetch.set_method(match request.method() {
            &Method::GET => abi_http_capnp::HttpMethod::Get,
            &Method::DELETE => abi_http_capnp::HttpMethod::Delete,
            &Method::OPTIONS => abi_http_capnp::HttpMethod::Options,
            &Method::PATCH => abi_http_capnp::HttpMethod::Patch,
            &Method::POST => abi_http_capnp::HttpMethod::Post,
            &Method::PUT => abi_http_capnp::HttpMethod::Put,
            other => todo!("http method not supported: {other:?}"),
        });
        fetch.set_uri(request.uri().to_string());
        capnp::serialize::write_message_to_words(&message)
    };

    let response_resource = OwnedResourceId::from_ffi(
        unsafe { fx_fetch(fetch.as_ptr() as u64, fetch.len() as u64) }
    );

    Ok(FutureHostResource::<HttpResponse>::new(response_resource).await)
}

impl DeserializeHostResource for HttpResponse {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<fx_types::abi_http_capnp::http_response::Reader>().unwrap();

        HttpResponse {
            status: http::StatusCode::from_u16(request.get_status()).unwrap(),
            body: request.get_body().unwrap().to_vec(),
        }
    }
}

#[derive(Debug, Error)]
pub enum FetchError {}

pub trait HttpResponseBody {
    fn into_bytes(self) -> Vec<u8>;
}

impl HttpResponseBody for Vec<u8> {
    fn into_bytes(self) -> Vec<u8> { self }
}

impl HttpResponseBody for String {
    fn into_bytes(self) -> Vec<u8> { self.into_bytes() }
}

impl HttpResponseBody for &str {
    fn into_bytes(self) -> Vec<u8> { self.as_bytes().to_vec() }
}
