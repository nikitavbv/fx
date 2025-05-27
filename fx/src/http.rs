use {
    fx_core::{HttpRequest, HttpResponse, FxStream, FxFutureError},
    crate::{
        fx_streams::FxStreamExport,
        fx_futures::{FxHostFuture, PoolIndex},
        sys,
    },
};

pub trait FxHttpRequest {
    fn with_body(self, body: Vec<u8>) -> Self;
    fn with_json(self, body: &serde_json::Value) -> Self;
}

impl FxHttpRequest for HttpRequest {
    fn with_body(self, body: Vec<u8>) -> Self {
        self.with_body_stream(FxStream::wrap(futures::stream::once(async { body })))
    }

    fn with_json(self, body: &serde_json::Value) -> Self {
        self.with_body(serde_json::to_vec(body).unwrap())
    }
}

pub async fn fetch(req: HttpRequest) -> Result<HttpResponse, FxFutureError> {
    let req = rmp_serde::to_vec(&req).unwrap();
    let future_index = unsafe { sys::fetch(req.as_ptr() as i64, req.len() as i64) };

    let response = FxHostFuture::new(PoolIndex(future_index as u64)).await?;
    Ok(rmp_serde::from_slice(&response).unwrap())
}
