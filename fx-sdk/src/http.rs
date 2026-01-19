use {
    fx_common::{HttpRequest, HttpResponse, FxStream, FxFutureError, HttpRequestError},
    fx_types::{capnp, abi_capnp},
    axum::http,
    thiserror::Error,
    crate::{
        fx_streams::FxStreamExport,
        fx_futures::{FxHostFuture, PoolIndex, HostFutureError, HostFuturePollRuntimeError, HostFutureAsyncApiError},
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

#[derive(Error, Debug)]
pub enum FetchError {
    /// fetch failed because of error in runtime implementation
    /// Should never happen. If you see this error it means there is a bug somewhere.
    #[error("error in runtime implementation: {0:?}")]
    RuntimeError(#[from] FetchRuntimeError),

    /// Problems with network connection caused this request to fail
    #[error("network error")]
    NetworkError,
}

#[derive(Error, Debug)]
pub enum FetchRuntimeError {
    #[error("failed to poll future: {0:?}")]
    FutureError(HostFuturePollRuntimeError),

    #[error("received unexpected async api response")]
    UnexpectedAsyncApiError,
}

pub async fn fetch(req: HttpRequest) -> Result<HttpResponse, FetchError> {
    let future_index = {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<abi_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut fetch_request = op.init_fetch();

        fetch_request.set_method(match req.method {
            http::Method::GET => abi_capnp::HttpMethod::Get,
            http::Method::POST => abi_capnp::HttpMethod::Post,
            http::Method::PUT => abi_capnp::HttpMethod::Put,
            http::Method::PATCH => abi_capnp::HttpMethod::Patch,
            http::Method::DELETE => abi_capnp::HttpMethod::Delete,
            _ => panic!("this http method is not supported"),
        });

        fetch_request.set_url(req.url.to_string());

        if let Some(body) = req.body.as_ref() {
            let mut request_body = fetch_request.init_body();
            request_body.set_id(body.index as u64);
        }

        let response = invoke_fx_api(message);
        let response = response.get_root::<abi_capnp::fx_api_call_result::Reader>().unwrap();
        match response.get_op().which().unwrap() {
            abi_capnp::fx_api_call_result::op::Which::Fetch(v) => {
                let fetch_response = v.unwrap();
                match fetch_response.get_response().which().unwrap() {
                    abi_capnp::fetch_response::response::Which::FutureId(v) => v,
                    abi_capnp::fetch_response::response::Which::FetchError(err) => panic!("fetch error: {}", err.unwrap().to_string().unwrap()),
                }
            },
            _other => panic!("unexpected response from fetch api"),
        }
    };

    let response = FxHostFuture::new(PoolIndex(future_index as u64)).await
        .map_err(|err| match err {
            HostFutureError::RuntimeError(err) => FetchError::RuntimeError(FetchRuntimeError::FutureError(err)),
            HostFutureError::AsyncApiError(err) => match err {
                HostFutureAsyncApiError::Fetch(_) => FetchError::NetworkError,
                _ => FetchError::RuntimeError(FetchRuntimeError::UnexpectedAsyncApiError),
            }
        })?;
    Ok(rmp_serde::from_slice(&response).unwrap())
}
