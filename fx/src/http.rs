use {
    fx_core::{HttpRequest, FxStream},
    crate::fx_streams::FxStreamExport,
};

pub trait FxHttpRequest {
    fn with_body(self, body: Vec<u8>) -> Self;
    fn with_json(self, body: serde_json::Value) -> Self;
}

impl FxHttpRequest for HttpRequest {
    fn with_body(self, body: Vec<u8>) -> Self {
        self.with_body_stream(FxStream::wrap(futures::stream::once(async { body })))
    }

    fn with_json(self, body: serde_json::Value) -> Self {
        self.with_body(serde_json::to_vec(&body).unwrap())
    }
}
