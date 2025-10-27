use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
    thiserror::Error,
    http::{HeaderMap, header::{IntoHeaderName, HeaderValue}, StatusCode, Method as HttpMethod, Uri},
};

pub mod api;

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpRequestInternal {
    #[serde(with = "http_serde::method")]
    pub method: HttpMethod,
    #[serde(with = "http_serde::uri")]
    pub url: Uri,
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap,
    pub body: Option<FxStream>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpRequest {
    #[serde(with = "http_serde::method")]
    pub method: HttpMethod,
    #[serde(with = "http_serde::uri")]
    pub url: Uri,
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap,
    pub body: Option<FxStream>,
}

impl HttpRequest {
    pub fn get(endpoint: impl Into<String>) -> Result<Self, HttpRequestError> {
        Self::new().with_method(HttpMethod::GET).with_url(endpoint)
    }

    pub fn post(endpoint: impl Into<String>) -> Result<Self, HttpRequestError> {
        Self::new().with_method(HttpMethod::POST).with_url(endpoint)
    }

    pub fn put(endpoint: impl Into<String>) -> Result<Self, HttpRequestError> {
        Self::new().with_method(HttpMethod::PUT).with_url(endpoint)
    }

    pub fn new() -> Self {
        Self {
            method: HttpMethod::GET,
            url: "/".parse().unwrap(),
            headers: HeaderMap::new(),
            body: None,
        }
    }

    pub fn with_method(mut self, method: HttpMethod) -> Self {
        self.method = method;
        self
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Result<Self, HttpRequestError> {
        self.url = url.into().parse()
            .map_err(|err| HttpRequestError::InvalidRequest { reason: format!("failed to parse url: {err:?}") })?;
        Ok(self)
    }

    pub fn with_query_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key: String = key.into();
        let key = urlencoding::encode(key.as_str());

        let value: String = value.into();
        let value = urlencoding::encode(value.as_str());

        let mut parts = self.url.clone().into_parts();
        let path_and_query = if let Some(existing) = &parts.path_and_query {
            let existing_str = existing.as_str();
            if existing_str.find('?').is_some() {
                format!("{}&{}={}", &existing_str[..], key, value)
            } else {
                format!("{}?{}={}", existing_str, key, value)
            }
        } else {
            format!("/?{}={}", key, value)
        };
        parts.path_and_query = Some(path_and_query.parse().unwrap());
        self.url = Uri::from_parts(parts).unwrap();
        self
    }

    pub fn with_header<K: IntoHeaderName, V: TryInto<HeaderValue>>(mut self, header: K, value: V) -> Result<Self, HttpRequestError> {
        self.headers.append(
            header,
            value.try_into().map_err(|_err| HttpRequestError::InvalidRequest {
                reason: "failed to convert into HeaderValue".to_owned(),
            })?
        );
        Ok(self)
    }

    pub fn with_body_stream(mut self, stream: FxStream) -> Self {
        self.body = Some(stream);
        self
    }

    pub fn into_internal(self) -> HttpRequestInternal {
        HttpRequestInternal { method: self.method, url: self.url, headers: self.headers, body: self.body }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpResponse {
    #[serde(with = "http_serde::status_code")]
    pub status: StatusCode,
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new() -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: vec![],
        }
    }

    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn with_header<K: IntoHeaderName>(mut self, header_name: K, header_value: impl Into<HeaderValue>) -> Self {
        self.headers.insert(header_name, header_value.into());
        self
    }

    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_body(mut self, body: impl HttpResponseBody) -> Self {
        self.body = body.into_bytes();
        self
    }
}

#[derive(Debug, Error)]
pub enum HttpRequestError {
    #[error("http request is invalid: {reason}")]
    InvalidRequest { reason: String },

    #[error("stream error")]
    StreamError { reason: String },
}

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

#[derive(Debug, Serialize, Deserialize)]
pub struct LogMessage {
    pub level: LogLevel,
    pub fields: HashMap<String, String>,
    pub event_type: LogEventType,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum LogEventType {
    Begin,
    End,
    Instant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseSqlQuery {
    pub database: String,
    pub query: SqlQuery,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseSqlBatchQuery {
    pub database: String,
    pub queries: Vec<SqlQuery>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SqlQuery {
    pub stmt: String,
    pub params: Vec<SqlValue>,
}

pub trait IntoQueryParam {
    fn into_query_param(self) -> SqlValue;
}

impl IntoQueryParam for i64 {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Integer(self)
    }
}

impl IntoQueryParam for &str {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Text(self.to_owned())
    }
}

impl IntoQueryParam for String {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Text(self)
    }
}

impl IntoQueryParam for Vec<u8> {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Blob(self)
    }
}

impl IntoQueryParam for bool {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Integer(if self { 1 } else { 0 })
    }
}

impl<T: IntoQueryParam> IntoQueryParam for Option<T> {
    fn into_query_param(self) -> SqlValue {
        match self {
            Some(v) => v.into_query_param(),
            None => SqlValue::Null,
        }
    }
}

impl SqlQuery {
    pub fn new(stmt: impl Into<String>) -> Self {
        Self {
            stmt: stmt.into(),
            params: Vec::new(),
        }
    }

    pub fn bind(mut self, param: impl IntoQueryParam) -> Self {
        self.params.push(param.into_query_param());
        self
    }
}

#[derive(Serialize, Deserialize)]
pub struct SqlResult {
    pub rows: Vec<SqlResultRow>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SqlResultRow {
    pub columns: Vec<SqlValue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum FxSqlError {
    #[error("requested database binding does not exist")]
    BindingNotExists,

    #[error("migration failed: {reason}")]
    MigrationFailed { reason: String },

    #[error("query failed: {reason}")]
    QueryFailed { reason: String },

    #[error("serialization error: {reason}")]
    SerializationError { reason: String },
}

#[derive(Error, Debug)]
pub enum SqlMappingError {
    #[error("this column cannot be converted to this type")]
    WrongType,
}

impl TryInto<String> for SqlValue {
    type Error = SqlMappingError;
    fn try_into(self) -> Result<String, Self::Error> {
        match self {
            Self::Text(text) => Ok(text),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryFrom<&SqlValue> for String {
    type Error = SqlMappingError;
    fn try_from(value: &SqlValue) -> Result<Self, Self::Error> {
        match value {
            SqlValue::Text(text) => Ok(text.clone()),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryFrom<&SqlValue> for u64 {
    type Error = SqlMappingError;
    fn try_from(value: &SqlValue) -> Result<Self, Self::Error> {
        match value {
            SqlValue::Integer(v) => Ok(*v as u64),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct SqlMigrations {
    pub database: String,
    pub migrations: Vec<String>,
}

// TODO: FxStream should create stream on host side. On host side, each entry in StreamPool should contain reference to ExecutionContext or wrapped stream
// created on host side (i.e., body of `fetch`). when stream is polled, host should run poll_next function in that ExecutionContext or poll_next a local Stream (depending on where stream lives).
// One more positive aspect of this implementation is that it allows passthrough of streams (for example, from fetch to HttpResponse) - poll_next on host will poll Stream on host side without
// invoking function and copying data there.
#[derive(Debug, Serialize, Deserialize)]
pub struct FxStream {
    // index in host and local pool
    pub index: i64,
}

#[derive(Error, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FxExecutionError {
    #[error("failed to read rpc request: {reason}")]
    RpcRequestRead { reason: String }
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum FxFutureError {
    #[error("rpc call failed: {reason}")]
    RpcError { reason: String },
    #[error("fetch call failed: {reason}")]
    FetchError { reason: String },
    #[error("serialization error: {reason}")]
    SerializationError { reason: String },
    #[error("fx runtime error: {reason}")]
    FxRuntimeError { reason: String },
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum FxStreamError {
    #[error("poll failed: {reason}")]
    PollFailed { reason: String },

    #[error("push failed: {reason}")]
    PushFailed { reason: String },
}

#[derive(Serialize, Deserialize)]
pub struct QueueMessage {
    pub data: Vec<u8>,
}

pub mod metrics {
    use super::*;

    #[derive(Serialize, Deserialize)]
    pub struct CounterIncrementRequest {
        pub counter_name: String,
        pub delta: u64,
        pub tags: Vec<(String, String)>,
    }
}
