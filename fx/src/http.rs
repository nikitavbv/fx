use {
    fx_common::{HttpRequest, HttpResponse, FxStream, FxFutureError, HttpRequestError},
    fx_api::{capnp, fx_capnp},
    axum::http,
    crate::{
        fx_streams::FxStreamExport,
        fx_futures::{FxHostFuture, PoolIndex},
        sys,
        invoke_fx_api,
    },
};

pub trait FxHttpRequest {
    fn with_body(self, body: Vec<u8>) -> Result<Self, HttpRequestError> where Self: Sized;
    fn with_json(self, body: &serde_json::Value) -> Result<Self, HttpRequestError> where Self: Sized;
}

impl FxHttpRequest for HttpRequest {
    fn with_body(self, body: Vec<u8>) -> Result<Self, HttpRequestError> {
        Ok(self.with_body_stream(
            FxStream::wrap(futures::stream::once(async { body }))
                .map_err(|err| HttpRequestError::StreamError {
                    reason: err.to_string(),
                })?
        ))
    }

    fn with_json(self, body: &serde_json::Value) -> Result<Self, HttpRequestError> {
        self
            .with_header("content-type", "application/json")?
            .with_body(serde_json::to_vec(body).unwrap())
    }
}

pub async fn fetch(req: HttpRequest) -> Result<HttpResponse, FxFutureError> {
    let future_index = {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut fetch_request = op.init_fetch();

        fetch_request.set_method(match req.method {
            http::Method::GET => fx_capnp::HttpMethod::Get,
            http::Method::POST => fx_capnp::HttpMethod::Post,
            http::Method::PUT => fx_capnp::HttpMethod::Put,
            http::Method::PATCH => fx_capnp::HttpMethod::Patch,
            http::Method::DELETE => fx_capnp::HttpMethod::Delete,
            _ => panic!("this http method is not supported"),
        });

        fetch_request.set_url(req.url.to_string());

        if let Some(body) = req.body.as_ref() {
            let mut request_body = fetch_request.init_body();
            request_body.set_id(body.index as u64);
        }

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();
        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::Fetch(v) => {
                let fetch_response = v.unwrap();
                match fetch_response.get_response().which().unwrap() {
                    fx_capnp::fetch_response::response::Which::FutureId(v) => v,
                    _other => panic!("fetch error"),
                }
            },
            _other => panic!("unexpected response from fetch api"),
        }
    };

    let response = FxHostFuture::new(PoolIndex(future_index as u64)).await?;
    Ok(rmp_serde::from_slice(&response).unwrap())
}
