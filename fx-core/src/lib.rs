use bincode::{Encode, Decode};

#[derive(Debug, Encode, Decode)]
pub struct HttpRequest {
    pub url: String,
}

#[derive(Debug, Encode, Decode)]
pub struct HttpResponse {
    pub body: String,
}
