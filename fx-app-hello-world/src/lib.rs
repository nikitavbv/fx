use fx::{HttpRequest, HttpResponse, handler};

#[handler]
pub fn handle(req: HttpRequest) -> HttpResponse {
    HttpResponse {
        body: format!("Hello 2 from {:?}", req.url),
    }
}
