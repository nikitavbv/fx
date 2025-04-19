use bincode::{Encode, Decode};

#[derive(Encode, Decode)]
pub struct HttpResponse {
    pub body: String,
}
