use {
    std::convert::Infallible,
    thiserror::Error,
    futures::FutureExt,
    http_body_util::BodyExt,
    bytes::Bytes,
    fx_types::{capnp, abi_blob_capnp},
    crate::{
        function::{
            abi::{function_memory::{FunctionMemoryError, FunctionMemoryAccessError, FunctionMemoryGetStringError}},
            instance::FunctionInstanceState,
        },
        triggers::http::HttpBody,
        tasks::blob::BlobMessage,
    },
};

#[derive(Debug, Error)]
pub(crate) enum BlobPutError {
    #[error("error in storage implementation")]
    StorageError,
}

impl From<crate::tasks::blob::PutError> for BlobPutError {
    fn from(err: crate::tasks::blob::PutError) -> Self {
        use crate::tasks::blob::PutError as SourceError;
        match err {
            SourceError::BlobStorageError => Self::StorageError,
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum BlobGetError {
    #[error("bad request: failed to access memory")]
    BadRequestFailedToAccessMemory,
    #[error("bad request: argument out of bounds")]
    BadRequestArgumentOutOfBounds,
    #[error("bad request: argument failed to decode")]
    BadRequestArgumentFailedToDecode,

    #[error("binding does not exist")]
    BindingNotExists,

    #[error("error in storage implementation")]
    StorageError,
}

impl From<FunctionMemoryError> for Result<Option<Vec<u8>>, BlobGetError> {
    fn from(value: FunctionMemoryError) -> Self {
        match value {
            FunctionMemoryError::MemoryNotFound | FunctionMemoryError::MemoryNotMemory => Err(BlobGetError::BadRequestFailedToAccessMemory),
        }
    }
}

impl From<FunctionMemoryAccessError> for Result<Option<Vec<u8>>, BlobGetError> {
    fn from(value: FunctionMemoryAccessError) -> Self {
        match value {
            FunctionMemoryAccessError::OutOfBounds => Err(BlobGetError::BadRequestArgumentOutOfBounds),
        }
    }
}

impl From<FunctionMemoryGetStringError> for Result<Option<Vec<u8>>, BlobGetError> {
    fn from(value: FunctionMemoryGetStringError) -> Self {
        match value {
            FunctionMemoryGetStringError::OutOfBounds => Err(BlobGetError::BadRequestArgumentOutOfBounds),
            FunctionMemoryGetStringError::FailedToDecode => Err(BlobGetError::BadRequestArgumentFailedToDecode),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum BlobDeleteError {
    #[error("failed to delete object because of unexpected error in blob storage implementation")]
    StorageError,
}

impl From<crate::tasks::blob::DeleteError> for BlobDeleteError {
    fn from(err: crate::tasks::blob::DeleteError) -> Self {
        use crate::tasks::blob::DeleteError as SourceError;
        match err {
            SourceError::BlobStorageError => Self::StorageError,
        }
    }
}

pub(crate) fn handle_blob_request(state: &FunctionInstanceState, req: http::Request<HttpBody>) -> futures::future::LocalBoxFuture<'static, http::Response<HttpBody>> {
    match req.uri().path() {
        "/delete" => {
            let bindings = state.bindings.blob.clone();
            let blob_tx = state.runtime_services.blob.clone();

            async move {
                let bytes: Bytes = req.into_body().collect().await.unwrap().to_bytes();

                let request_reader = capnp::serialize::read_message_from_flat_slice(&mut bytes.as_ref(), capnp::message::ReaderOptions::default()).unwrap();
                let request = request_reader.get_root::<abi_blob_capnp::blob_delete_request::Reader>().unwrap();

                let binding = request.get_binding().unwrap().to_str().unwrap();
                let bucket = bindings.get(binding).unwrap().bucket.clone();

                let key = request.get_key().unwrap().to_vec();

                let (result, result_rx) = tokio::sync::oneshot::channel();
                blob_tx.send_async(BlobMessage::Delete { bucket, key, result }).await.unwrap();
                let result = result_rx.await.unwrap().map_err(BlobDeleteError::from);

                let mut message = capnp::message::Builder::new_default();
                let blob_delete_response = message.init_root::<abi_blob_capnp::blob_delete_response::Builder>();
                let mut response = blob_delete_response.init_response();

                match result {
                    Ok(()) => response.set_ok(()),
                    Err(BlobDeleteError::StorageError) => response.set_storage_error(()),
                }

                let bytes = capnp::serialize::write_message_to_words(&message);

                http::Response::new(HttpBody::for_bytes(bytes.into()))
            }.boxed_local()
        },
        _other => {
            let mut response = http::Response::new(HttpBody::for_bytes("not found.\n".into()));
            *response.status_mut() = http::StatusCode::NOT_FOUND;
            std::future::ready(response).boxed_local()
        },
    }
}
