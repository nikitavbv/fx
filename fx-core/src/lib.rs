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
