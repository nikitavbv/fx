use {
    fx::{FxCtx, HttpRequest, HttpResponse, rpc},
    tracing::info,
    axum::{Router, routing::{RouterIntoService, get}, body::{Body, Bytes}, http::Request},
    tower::Service,
    futures::StreamExt,
    fx_utils::block_on,
};

#[rpc]
pub fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    info!("handling request");

    let mut service: RouterIntoService<Body>  = Router::new()
        .route("/", get(home))
        .route("/something", get(something))
        .into_service();

    let res = service.call(Request::builder().uri(req.url).body(Body::empty()).unwrap());

    let response = block_on(async move {
        let res = res.await.unwrap();

        let body = res.into_body();
        let mut stream = body.into_data_stream();

        let mut whole_body: Vec<u8> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            whole_body.append(&mut chunk.to_vec());
        }

        whole_body
    });

    info!("response: {response:?}");

    HttpResponse::new().body(String::from_utf8(response).unwrap())
}

async fn home() -> &'static str {
    "hello from dashboard home!"
}

async fn something() -> &'static str {
    "something"
}
