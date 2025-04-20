use fx::{FxCtx, HttpRequest, HttpResponse, handler};

#[handler]
pub fn handle(_ctx: FxCtx, req: HttpRequest) -> HttpResponse {
    HttpResponse {
        body: format!("Hello 2 from {:?}", req.url),
    }
}
