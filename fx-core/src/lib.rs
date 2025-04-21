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
    pub body: String,
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
