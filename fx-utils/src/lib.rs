use {
    fx::{HttpRequest, HttpResponse},
    axum::{http::Request, body::Body},
    tower::Service,
    futures::StreamExt,
};

pub use wasm_rs_async_executor::single_threaded::block_on;

pub fn handle_http_axum_router(router: axum::Router, req: HttpRequest) -> HttpResponse {
    let mut service = router.into_service();

    let response = service.call(Request::builder().uri(req.url).body(Body::empty()).unwrap());

    let response = block_on(async move {
        let res = response.await.unwrap();

        let body = res.into_body();
        let mut stream = body.into_data_stream();

        let mut whole_body: Vec<u8> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            whole_body.append(&mut chunk.to_vec());
        }

        whole_body
    });

    HttpResponse::new().body(String::from_utf8(response).unwrap())
}
