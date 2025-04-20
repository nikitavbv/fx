use {
    std::collections::HashMap,
    bincode::{Encode, Decode},
};

#[derive(Debug, Encode, Decode)]
pub struct HttpRequest {
    pub url: String,
}

#[derive(Debug, Encode, Decode)]
pub struct HttpResponse {
    pub body: String,
}

#[derive(Debug, Encode, Decode)]
pub struct LogMessage {
    pub fields: HashMap<String, String>,
}
