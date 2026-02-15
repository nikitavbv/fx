use {
    axum::{http::Request, body::Body},
    tower::Service,
    futures::StreamExt,
    crate::{HttpRequest, HttpResponse},
};

pub async fn handle_request(router: axum::Router, src_req: HttpRequest) -> HttpResponse {
    let mut service = router.into_service();

    // let body = src_req.body.map(|body| body.import());

    let request = Request::builder()
        .uri(src_req.uri())
        .method(src_req.method());
    /*for (k, v) in src_req.headers {
        if let Some(k) = k {
            request = request.header(k, v);
        }
    }*/

    let fx_response = service.call(request
        .body(Body::empty())
        //.body(body.map(Body::from_stream).unwrap_or(Body::empty()))
        .unwrap()
    );
    let fx_response = fx_response.await.unwrap();
    let response = HttpResponse::new()
        .with_status(fx_response.status());
    //.with_headers(fx_response.headers().clone());

    let body = fx_response.into_body();
    let mut stream = body.into_data_stream();

    let mut response_body: Vec<u8> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        response_body.append(&mut chunk.to_vec());
    }

    response.with_body(response_body)
}
