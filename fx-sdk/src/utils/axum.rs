use {
    axum::{http::Request, body::Body},
    tower::Service,
    futures::{StreamExt, TryStreamExt},
    crate::{HttpRequest, HttpResponse},
};

pub async fn handle_request(router: axum::Router, mut src_req: HttpRequest) -> HttpResponse {
    let mut service = router.into_service();

    let mut request = Request::builder()
        .uri(src_req.uri())
        .method(src_req.method());
    for (k, v) in src_req.headers() {
        request = request.header(k, v);
    }

    let request = request.body(src_req.body().unwrap_or_default()).unwrap();

    let fx_response = service.call(request);
    let fx_response = fx_response.await.unwrap();

    let (parts, body) = fx_response.into_parts();
    let response = HttpResponse::from_parts(parts);
    let stream = body.into_data_stream();

    response.with_body(stream.map_err(|err| {
        panic!("unexpected stream error: {err:?}");
        ()
    }).boxed())
}
