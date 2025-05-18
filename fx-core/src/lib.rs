use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
    thiserror::Error,
    http::{HeaderMap, header::{IntoHeaderName, HeaderValue}, StatusCode, Method as HttpMethod},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpRequest {
    pub url: String,
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap,
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
    pub fields: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FetchRequest {
    pub endpoint: String,
    #[serde(with = "http_serde::method")]
    pub method: HttpMethod,
    #[serde(with = "http_serde::header_map")]
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
}

impl FetchRequest {
    pub fn get(endpoint: impl Into<String>) -> Self { Self::with_method(HttpMethod::GET, endpoint.into()) }
    pub fn post(endpoint: impl Into<String>) -> Self { Self::with_method(HttpMethod::POST, endpoint.into()) }
    pub fn put(endpoint: impl Into<String>) -> Self { Self::with_method(HttpMethod::PUT, endpoint.into()) }
    fn with_method(method: HttpMethod, endpoint: String) -> Self {
        Self {
            endpoint,
            method,
            headers: HeaderMap::new(),
            body: None,
        }
    }

    pub fn with_header<K: IntoHeaderName>(mut self, header_name: K, header_value: impl Into<HeaderValue>) -> Self {
        self.headers.insert(header_name, header_value.into());
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    pub fn with_json(self, json: &serde_json::Value) -> Self {
        self
            .with_header("content-type", HeaderValue::from_static("application/json"))
            .with_body(serde_json::to_vec(json).unwrap())
    }
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

impl IntoQueryParam for String {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Text(self)
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

#[derive(Debug, Serialize, Deserialize)]
pub struct CronRequest {}

// TODO: FxStream should create stream on host side. On host side, each entry in StreamPool should contain reference to ExecutionContext or wrapped stream
// created on host side (i.e., body of `fetch`). when stream is polled, host should run poll_next function in that ExecutionContext or poll_next a local Stream (depending on where stream lives).
// One more positive aspect of this implementation is that it allows passthrough of streams (for example, from fetch to HttpResponse) - poll_next on host will poll Stream on host side without
// invoking function and copying data there.
#[derive(Serialize, Deserialize)]
pub struct FxStream {
    // index in host and local pool
    pub index: i64,
}

#[derive(Error, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FxExecutionError {
    #[error("failed to read rpc request: {reason}")]
    RpcRequestRead { reason: String }
}
