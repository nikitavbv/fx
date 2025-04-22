use fx::{FxCtx, HttpRequest, HttpResponse, rpc};

#[rpc]
pub fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    HttpResponse::new().body("hello from dashboard")
}
