use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpRequest {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

impl HttpResponse {
    pub fn new() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: None,
        }
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub fn header(mut self, header_name: impl Into<String>, header_value: impl Into<String>) -> Self {
        self.headers.insert(header_name.into(), header_value.into());
        self
    }

    pub fn headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    pub fn body(mut self, body: impl HttpResponseBody) -> Self {
        self.body = Some(body.into_bytes());
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
    pub method: HttpMethod,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

impl FetchRequest {
    pub fn get(endpoint: String) -> Self { Self::with_method(HttpMethod::GET, endpoint) }
    pub fn post(endpoint: String) -> Self { Self::with_method(HttpMethod::POST, endpoint) }
    fn with_method(method: HttpMethod, endpoint: String) -> Self {
        Self {
            endpoint,
            method,
            headers: HashMap::new(),
            body: None,
        }
    }

    pub fn with_header(mut self, header_name: String, header_value: String) -> Self {
        self.headers.insert(header_name.to_owned(), header_value.to_owned());
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum HttpMethod {
    GET,
    POST,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FetchResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseSqlQuery {
    pub database: String,
    pub query: SqlQuery,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SqlQuery {
    pub stmt: String,
    pub params: Vec<QueryParam>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum QueryParam {
    Null,
}

pub trait IntoQueryParam {
    fn into_query_param(self) -> QueryParam;
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

#[derive(Debug, Serialize, Deserialize)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
