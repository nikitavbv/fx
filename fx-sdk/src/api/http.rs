pub use http::{HeaderName, HeaderValue, Uri};

use {
    std::str::FromStr,
    http::{Method, HeaderMap},
    serde::Serialize,
    thiserror::Error,
    futures::{StreamExt, TryStreamExt, stream::{BoxStream, Stream}},
    bytes::Bytes,
    fx_types::{capnp, abi_http_capnp, abi::FuturePollResult},
    crate::sys::{
        ResourceId,
        DeserializableHostResource,
        DeserializeHostResource,
        SerializeResource,
        SerializableResource,
        OwnedResourceId,
        FutureHostResource,
        fx_fetch,
        fx_future_poll,
        fx_resource_serialize,
        fx_stream_frame_read,
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

    pub fn post(url: impl Into<String>) -> Result<Self, ()> {
        Ok(Self::new(Method::POST, url.into().parse().unwrap()))
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

    fn request_data_mut(&mut self) -> &mut HttpRequestData {
        match &mut self.0 {
            HttpRequestInner::Host(v) => v.get_raw_mut(),
            HttpRequestInner::Function(v) => v,
        }
    }

    pub fn method(&self) -> &Method {
        &self.request_data().method
    }

    pub fn uri(&self) -> &Uri {
        &self.request_data().url
    }

    pub fn with_uri(mut self, uri: Uri) -> Self {
        self.request_data_mut().url = uri;
        self
    }

    pub fn with_query(mut self, query: &impl Serialize) -> Self {
        let query_string = serde_urlencoded::to_string(query).unwrap();

        let uri = self.request_data().url.clone();
        let mut parts = uri.into_parts();

        let new_path_and_query = match parts.path_and_query {
            Some(ref pq) => {
                match pq.query() {
                    Some(existing) => format!("{}?{}&{}", pq.path(), existing, query_string),
                    None => format!("{}?{}", pq.path(), query_string),
                }
            },
            None => format!("/?{}", query_string),
        };

        parts.path_and_query = Some(new_path_and_query.parse().unwrap());
        self.request_data_mut().url = Uri::from_parts(parts).unwrap();
        self
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.request_data().headers
    }

    pub fn with_body(mut self, body: impl IntoHttpBody) -> Self {
        self.request_data_mut().body = Some(body.into_http_body().0);
        self
    }

    pub fn body(&mut self) -> Option<HttpBody> {
        self.request_data_mut().read_body().map(|v| HttpBody(v))
    }

    fn body_into_bytes(&mut self) -> Option<Vec<u8>> {
        if let Some(v) = self.body() {
            v.read_all()
        } else {
            None
        }
    }
}

pub(crate) struct HttpRequestData {
    method: Method,
    url: Uri,
    headers: HeaderMap,
    body: Option<HttpBodyInner>,
}

pub struct HttpRequestBody(HttpRequestBodyInner);

impl HttpRequestBody {
    fn read_all(self) -> Option<Vec<u8>> {
        match self.0 {
            HttpRequestBodyInner::Empty => None,
            HttpRequestBodyInner::Bytes(v) => Some(v),
            HttpRequestBodyInner::HostResource(v) => todo!("host resource reading into bytes vec"),
        }
    }
}

impl Default for HttpRequestBody {
    fn default() -> Self {
        Self(HttpRequestBodyInner::Empty)
    }
}

impl http_body::Body for HttpRequestBody {
    type Data = bytes::Bytes;
    type Error = ReadBodyError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let body = std::mem::replace(&mut self.0, HttpRequestBodyInner::Empty);
        let (new_body, poll) = match body {
            HttpRequestBodyInner::Empty => (HttpRequestBodyInner::Empty, std::task::Poll::Ready(None)),
            HttpRequestBodyInner::Bytes(v) => (HttpRequestBodyInner::Empty, std::task::Poll::Ready(Some(Ok(http_body::Frame::data(bytes::Bytes::from(v)))))),
            HttpRequestBodyInner::HostResource(resource_id) => {
                let poll_result = resource_id.with(|resource_id| unsafe { fx_future_poll(resource_id.as_ffi()) });
                let poll_result = FuturePollResult::try_from(poll_result).unwrap();
                match poll_result {
                    FuturePollResult::Pending => (HttpRequestBodyInner::HostResource(resource_id), std::task::Poll::Pending),
                    FuturePollResult::Ready => {
                        let resource_length = resource_id.with(|resource_id| unsafe { fx_resource_serialize(resource_id.as_ffi()) });
                        let data: Vec<u8> = vec![0u8; resource_length as usize];
                        resource_id.with(|resource_id| unsafe { fx_stream_frame_read(resource_id.as_ffi(), data.as_ptr() as u64); });

                        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut data.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                        let request = resource_reader.get_root::<abi_http_capnp::http_request_body_frame::Reader>().unwrap();
                        match request.get_body().which().unwrap() {
                            abi_http_capnp::http_request_body_frame::body::Which::StreamEnd(_) => (HttpRequestBodyInner::Empty, std::task::Poll::Ready(None)),
                            abi_http_capnp::http_request_body_frame::body::Which::Bytes(v) => (
                                HttpRequestBodyInner::HostResource(resource_id),
                                std::task::Poll::Ready(Some(Ok(http_body::Frame::data(bytes::Bytes::from(v.unwrap().to_vec())))))
                            )
                        }
                    },
                }
            },
        };
        let _ = std::mem::replace(&mut self.0, new_body);
        poll
    }
}

#[derive(Debug, Error)]
pub enum ReadBodyError {}

pub(crate) enum HttpRequestBodyInner {
    Empty,
    Bytes(Vec<u8>),
    HostResource(OwnedResourceId),
}

impl HttpRequestData {
    pub fn new(method: Method, url: Uri) -> Self {
        Self {
            method,
            url,
            headers: HeaderMap::new(),
            body: None,
        }
    }

    fn read_body(&mut self) -> Option<HttpBodyInner> {
        std::mem::replace(&mut self.body, None)
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
                abi_http_capnp::HttpMethod::Head => Method::HEAD,
                abi_http_capnp::HttpMethod::Connect => Method::CONNECT,
                abi_http_capnp::HttpMethod::Trace => Method::TRACE,
            },
            url: Uri::from_str(request.get_uri().unwrap().to_str().unwrap()).unwrap(),
            headers: request.get_headers().unwrap().into_iter()
                .map(|header| (
                    HeaderName::from_bytes(header.get_name().unwrap().as_bytes()).unwrap(),
                    HeaderValue::from_bytes(header.get_value().unwrap().as_bytes()).unwrap()
                ))
                .collect(),
            body: match request.get_body().unwrap().get_body().which().unwrap() {
                abi_http_capnp::http_request_body::body::Which::Empty(_) => None,
                abi_http_capnp::http_request_body::body::Which::Bytes(v) => Some(HttpBodyInner::Bytes(v.unwrap().to_vec())),
                abi_http_capnp::http_request_body::body::Which::HostResource(v) => Some(HttpBodyInner::HostResource(OwnedResourceId::from_ffi(v))),
            },
        }
    }
}

pub struct HttpResponse {
    parts: http::response::Parts,
    body: HttpBody,
}

impl HttpResponse {
    pub fn new() -> Self {
        Self::from_parts(http::Response::builder().body(()).unwrap().into_parts().0)
    }

    pub fn from_parts(parts: http::response::Parts) -> Self {
        Self {
            parts,
            body: HttpBody::empty(),
        }
    }

    pub fn status(&self) -> &http::StatusCode {
        &self.parts.status
    }

    pub fn with_status(mut self, status: http::StatusCode) -> Self {
        self.parts.status = status;
        self
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.parts.headers
    }

    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.parts.headers.insert(name, value);
        self
    }

    pub fn into_parts(self) -> (http::response::Parts, HttpBody) {
        (self.parts, self.body)
    }

    pub fn into_body(self) -> HttpBody {
        self.body
    }

    pub fn with_body(mut self, body: impl Into<HttpBody>) -> Self {
        self.body = body.into();
        self
    }

    pub async fn bytes(self) -> Vec<u8> {
        self.into_body().map(|v| v.map(|v| v.to_vec())).try_concat().await.unwrap()
    }

    pub async fn text(self) -> String {
        String::from_utf8(self.bytes().await).unwrap()
    }
}

pub struct HttpBody(pub(crate) HttpBodyInner);

impl Default for HttpBody {
    fn default() -> Self {
        Self::empty()
    }
}

impl HttpBody {
    pub fn empty() -> Self {
        Self(HttpBodyInner::Empty)
    }

    pub fn bytes(bytes: Vec<u8>) -> Self {
        Self(HttpBodyInner::Bytes(bytes))
    }

    pub fn stream(stream: BoxStream<'static, Result<Bytes, HttpStreamError>>) -> Self {
        Self(HttpBodyInner::Stream(stream))
    }

    pub fn host_resource(resource_id: OwnedResourceId) -> Self {
        Self(HttpBodyInner::HostResource(resource_id))
    }

    pub fn read_all(self) -> Option<Vec<u8>> {
        match self.0 {
            HttpBodyInner::Empty => None,
            HttpBodyInner::Bytes(v) => Some(v),
            HttpBodyInner::HostResource(_) => todo!("host resource reading into bytes vec"),
            HttpBodyInner::Stream(_) => todo!("stream reading into bytes vec"),
            HttpBodyInner::PartiallyReadStream { .. } | HttpBodyInner::FrameSerialized(_) | HttpBodyInner::BytesSerialized(_) => panic!("resource of this type cannot be read"),
        }
    }
}

impl Stream for HttpBody {
    type Item = Result<Bytes, HttpBodyStreamError>;

    fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        let inner = std::mem::replace(&mut self.0, HttpBodyInner::Empty);

        let (inner, poll_result) = match inner {
            HttpBodyInner::Empty => (HttpBodyInner::Empty, std::task::Poll::Ready(None)),
            HttpBodyInner::Bytes(_) => todo!(),
            HttpBodyInner::BytesSerialized(_) => todo!(),
            HttpBodyInner::Stream(_) => todo!(),
            HttpBodyInner::PartiallyReadStream { .. } => todo!(),
            HttpBodyInner::HostResource(resource_id) => {
                let poll_result = resource_id.with(|resource_id| unsafe { fx_future_poll(resource_id.as_ffi()) });
                let poll_result = FuturePollResult::try_from(poll_result).unwrap();
                match poll_result {
                    FuturePollResult::Pending => (HttpBodyInner::HostResource(resource_id), std::task::Poll::Pending),
                    FuturePollResult::Ready => {
                        let resource_length = resource_id.with(|resource_id| unsafe { fx_resource_serialize(resource_id.as_ffi()) });
                        let data: Vec<u8> = vec![0u8; resource_length as usize];
                        resource_id.with(|resource_id| unsafe { fx_stream_frame_read(resource_id.as_ffi(), data.as_ptr() as u64); });

                        let frame_reader = capnp::serialize::read_message_from_flat_slice(&mut data.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                        let frame = frame_reader.get_root::<abi_http_capnp::http_body_frame::Reader>().unwrap();

                        match frame.get_frame().which().unwrap() {
                            abi_http_capnp::http_body_frame::frame::Which::StreamEnd(_) => (HttpBodyInner::Empty, std::task::Poll::Ready(None)),
                            abi_http_capnp::http_body_frame::frame::Which::Bytes(v) => (
                                HttpBodyInner::HostResource(resource_id),
                                std::task::Poll::Ready(Some(Ok(Bytes::from(v.unwrap().to_vec()))))
                            ),
                        }
                    },
                }
            },
            HttpBodyInner::FrameSerialized(_) => panic!("cannot read from HttpBody that has just been serialized for writing to host"),
        };

        self.0 = inner;

        poll_result
    }
}

impl http_body::Body for HttpBody {
    type Data = bytes::Bytes;
    type Error = ReadBodyError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let inner = std::mem::replace(&mut self.0, HttpBodyInner::Empty);

        let (inner, poll_result) = match inner {
            HttpBodyInner::Empty => (HttpBodyInner::Empty, std::task::Poll::Ready(None)),
            HttpBodyInner::Bytes(_) => todo!(),
            HttpBodyInner::BytesSerialized(_) => todo!(),
            HttpBodyInner::Stream(_) => todo!(),
            HttpBodyInner::PartiallyReadStream { .. } => todo!(),
            HttpBodyInner::HostResource(resource_id) => {
                let poll_result = resource_id.with(|resource_id| unsafe { fx_future_poll(resource_id.as_ffi()) });
                let poll_result = FuturePollResult::try_from(poll_result).unwrap();
                match poll_result {
                    FuturePollResult::Pending => (HttpBodyInner::HostResource(resource_id), std::task::Poll::Pending),
                    FuturePollResult::Ready => {
                        let resource_length = resource_id.with(|resource_id| unsafe { fx_resource_serialize(resource_id.as_ffi()) });
                        let data: Vec<u8> = vec![0u8; resource_length as usize];
                        resource_id.with(|resource_id| unsafe { fx_stream_frame_read(resource_id.as_ffi(), data.as_ptr() as u64); });

                        let frame_reader = capnp::serialize::read_message_from_flat_slice(&mut data.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                        let frame = frame_reader.get_root::<abi_http_capnp::http_body_frame::Reader>().unwrap();

                        match frame.get_frame().which().unwrap() {
                            abi_http_capnp::http_body_frame::frame::Which::StreamEnd(_) => (HttpBodyInner::Empty, std::task::Poll::Ready(None)),
                            abi_http_capnp::http_body_frame::frame::Which::Bytes(v) => (
                                HttpBodyInner::HostResource(resource_id),
                                std::task::Poll::Ready(Some(Ok(http_body::Frame::data(Bytes::from(v.unwrap().to_vec())))))
                            ),
                        }
                    },
                }
            },
            HttpBodyInner::FrameSerialized(_) => panic!("cannot read from HttpBody that has just been serialized for writing to host"),
        };

        self.0 = inner;

        poll_result
    }
}

#[derive(Debug, Error)]
pub enum HttpBodyStreamError {}

impl axum::response::IntoResponse for HttpBody {
    fn into_response(self) -> axum::response::Response {
        axum::response::Response::new(axum::body::Body::from_stream(self))
    }
}

pub(crate) enum HttpBodyInner {
    Empty,
    Bytes(Vec<u8>),
    BytesSerialized(Vec<u8>),
    Stream(BoxStream<'static, Result<Bytes, HttpStreamError>>),
    PartiallyReadStream {
        stream: BoxStream<'static, Result<Bytes, HttpStreamError>>,
        frame_serialized: Vec<u8>,
    },
    HostResource(OwnedResourceId),
    FrameSerialized(Vec<u8>),
}

pub(crate) fn serialize_http_body_full(body: Vec<u8>) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    let serialized_frame = message.init_root::<abi_http_capnp::function_http_body_frame::Builder>();
    let mut serialized_frame = serialized_frame.init_body();

    serialized_frame.set_bytes(&body);

    capnp::serialize::write_message_to_words(&message)
}

pub async fn fetch(mut request: HttpRequest) -> Result<HttpResponse, FetchError> {
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

        let mut request_body = fetch.init_body().init_body();
        match request.body() {
            Some(body) => match body.0 {
                HttpBodyInner::Empty => request_body.set_empty(()),
                HttpBodyInner::Bytes(v) => request_body.set_bytes(&v),
                HttpBodyInner::Stream(_) => todo!("using stream as request body is not supported yet"),
                HttpBodyInner::HostResource(resource_id) => request_body.set_host_resource(resource_id.consume().as_ffi()),
                HttpBodyInner::BytesSerialized(_) => panic!("http body of this type (BytesSerialized) cannot be used as request body"),
                HttpBodyInner::PartiallyReadStream { .. } => panic!("http body of this type (PartiallyReadStream) cannot be used as request body"),
                HttpBodyInner::FrameSerialized(_) => panic!("http body of this type (FrameSerialized) cannot be used as request body"),
            },
            None => request_body.set_empty(()),
        }

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

        let mut parts = http::response::Builder::new()
            .status(http::StatusCode::from_u16(request.get_status()).unwrap())
            .body(())
            .unwrap()
            .into_parts()
            .0;

        for header in request.get_headers().unwrap() {
            let name = HeaderName::from_bytes(header.get_name().unwrap().as_bytes()).unwrap();
            let value = HeaderValue::from_str(header.get_value().unwrap().to_str().unwrap()).unwrap();
            parts.headers.append(name, value);
        }

        HttpResponse {
            parts,
            body: HttpBody::host_resource(OwnedResourceId::from_ffi(request.get_body_resource_id())),
        }
    }
}

#[derive(Debug, Error)]
pub enum FetchError {}

pub trait IntoHttpBody {
    fn into_http_body(self) -> HttpBody;
}

impl IntoHttpBody for Vec<u8> {
    fn into_http_body(self) -> HttpBody {
        HttpBody(HttpBodyInner::Bytes(self))
    }
}

impl IntoHttpBody for &str {
    fn into_http_body(self) -> HttpBody {
        HttpBody(HttpBodyInner::Bytes(self.as_bytes().to_vec()))
    }
}

impl IntoHttpBody for String {
    fn into_http_body(self) -> HttpBody {
        HttpBody(HttpBodyInner::Bytes(self.as_bytes().to_vec()))
    }
}

impl IntoHttpBody for HttpBody {
    fn into_http_body(self) -> HttpBody {
        self
    }
}

pub trait IntoHttpRequestBody {
    fn into_request_body(self) -> HttpRequestBody;
}

impl IntoHttpRequestBody for Vec<u8> {
    fn into_request_body(self) -> HttpRequestBody {
        HttpRequestBody(HttpRequestBodyInner::Bytes(self))
    }
}

impl IntoHttpRequestBody for String {
    fn into_request_body(self) -> HttpRequestBody {
        HttpRequestBody(HttpRequestBodyInner::Bytes(self.as_bytes().to_vec()))
    }
}

impl IntoHttpRequestBody for &str {
    fn into_request_body(self) -> HttpRequestBody {
        HttpRequestBody(HttpRequestBodyInner::Bytes(self.as_bytes().to_vec()))
    }
}

impl IntoHttpRequestBody for HttpRequestBody {
    fn into_request_body(self) -> HttpRequestBody {
        self
    }
}

pub trait IntoHttpResponseBody {
    fn into_bytes(self) -> Vec<u8>;
}

impl IntoHttpResponseBody for Vec<u8> {
    fn into_bytes(self) -> Vec<u8> { self }
}

impl IntoHttpResponseBody for String {
    fn into_bytes(self) -> Vec<u8> { self.as_bytes().to_vec() }
}

impl IntoHttpResponseBody for &str {
    fn into_bytes(self) -> Vec<u8> { self.as_bytes().to_vec() }
}

impl From<BoxStream<'static, Result<Bytes, HttpStreamError>>> for HttpBody {
    fn from(stream: BoxStream<'static, Result<Bytes, HttpStreamError>>) -> Self {
        HttpBody::stream(stream)
    }
}

impl From<&str> for HttpBody {
    fn from(value: &str) -> Self {
        HttpBody(HttpBodyInner::Bytes(value.as_bytes().to_vec()))
    }
}

impl From<String> for HttpBody {
    fn from(value: String) -> Self {
        HttpBody(HttpBodyInner::Bytes(value.into_bytes()))
    }
}

#[derive(Debug, Error)]
pub(crate) enum HttpStreamError {
    #[error("failed to read axum stream: {0:?}")]
    AxumStreamRead(axum::Error),
}
