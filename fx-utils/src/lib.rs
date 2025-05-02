use {
    fx::{HttpRequest, HttpResponse},
    axum::{http::Request, body::Body},
    tower::Service,
    futures::StreamExt,
};

pub mod database;

pub async fn handle_http_axum_router(router: axum::Router, req: HttpRequest) -> HttpResponse {
    let mut service = router.into_service();

    let fx_response = service.call(Request::builder().uri(req.url).body(Body::empty()).unwrap());
    let fx_response = fx_response.await.unwrap();
    let response = HttpResponse::new()
        .status(fx_response.status().as_u16())
        .headers(fx_response.headers().into_iter().map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap().to_owned())).collect());

    let body = fx_response.into_body();
    let mut stream = body.into_data_stream();

    let mut response_body: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        response_body.append(&mut chunk.to_vec());
    }

    response.body(response_body)
}
