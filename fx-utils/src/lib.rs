use {
    fx::{HttpRequest, HttpResponse, FxStreamImport},
    axum::{http::Request, body::Body},
    tower::Service,
    futures::StreamExt,
};

pub mod database;

pub async fn handle_http_axum_router(router: axum::Router, req: HttpRequest) -> HttpResponse {
    let mut service = router.into_service();

    let body = req.body.import()
        .map(|v| Ok::<Vec<u8>, std::convert::Infallible>(v));

    let fx_response = service.call(Request::builder()
        .uri(req.url)
        .method(req.method)
        .body(Body::from_stream(body))
        .unwrap()
    );
    let fx_response = fx_response.await.unwrap();
    let response = HttpResponse::new()
        .with_status(fx_response.status())
        .with_headers(fx_response.headers().clone());

    let body = fx_response.into_body();
    let mut stream = body.into_data_stream();

    let mut response_body: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        response_body.append(&mut chunk.to_vec());
    }

    response.with_body(response_body)
}
