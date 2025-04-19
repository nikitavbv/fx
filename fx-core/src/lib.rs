use bincode::{Encode, Decode};

#[derive(Debug, Encode, Decode)]
pub struct HttpResponse {
    pub body: String,
}
