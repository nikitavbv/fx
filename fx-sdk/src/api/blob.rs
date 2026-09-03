use {
    fx_types::{capnp, abi_blob_capnp},
    thiserror::Error,
};

pub struct BlobBucket {
    binding: String,
}

impl BlobBucket {
    pub fn new(binding: impl Into<String>) -> Self {
        Self { binding: binding.into() }
    }

    pub async fn put(&self, key: String, value: Vec<u8>) -> Result<(), BlobPutError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_blob_capnp::blob_put_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_key(key.as_bytes());
            message_request.set_value(&value);

            capnp::serialize::write_message_to_words(&message)
        };

        let result_vec = crate::api::http::fetch(
            crate::HttpRequest::post("http://blob.fx.internal/put").unwrap()
                .with_body(request)
        ).await.unwrap().bytes().await;

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<abi_blob_capnp::blob_put_result::Reader>().unwrap();
        match request.get_result().which().unwrap() {
            abi_blob_capnp::blob_put_result::result::Which::Ok(()) => Ok(()),
            abi_blob_capnp::blob_put_result::result::Which::StorageError(()) => Err(BlobPutError::StorageError),
        }
    }

    pub async fn get(&self, key: String) -> Result<Option<Vec<u8>>, BlobGetError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_blob_capnp::blob_get_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_key(key.as_bytes());

            capnp::serialize::write_message_to_words(&message)
        };

        let result_vec = crate::api::http::fetch(
            crate::HttpRequest::post("http://blob.fx.internal/get").unwrap()
                .with_body(request)
        ).await.unwrap().bytes().await;

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<abi_blob_capnp::blob_get_response::Reader>().unwrap();
        match request.get_response().which().unwrap() {
            abi_blob_capnp::blob_get_response::response::Which::NotFound(_) => Ok(None),
            abi_blob_capnp::blob_get_response::response::Which::Value(v) => Ok(Some(v.unwrap().to_vec())),
            abi_blob_capnp::blob_get_response::response::Which::BindingNotExists(_) => Err(BlobGetError::BindingNotExists),
            abi_blob_capnp::blob_get_response::response::Which::BadRequestArgumentOutOfBounds(_)
            | abi_blob_capnp::blob_get_response::response::Which::BadRequestFailedToAccessMemory(_) => Err(BlobGetError::InternalSdkError),
            abi_blob_capnp::blob_get_response::response::Which::StorageError(()) => Err(BlobGetError::StorageError),
        }
    }

    pub async fn delete(&self, key: String) -> Result<(), BlobDeleteError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut message_request = message.init_root::<abi_blob_capnp::blob_delete_request::Builder>();
            message_request.set_binding(&self.binding);
            message_request.set_key(key.as_bytes());

            capnp::serialize::write_message_to_words(&message)
        };

        let result_vec = crate::api::http::fetch(
            crate::HttpRequest::post("http://blob.fx.internal/delete").unwrap()
                .with_body(request)
        ).await.unwrap().bytes().await;

        let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<abi_blob_capnp::blob_delete_response::Reader>().unwrap();
        match request.get_response().which().unwrap() {
            abi_blob_capnp::blob_delete_response::response::Which::Ok(()) => Ok(()),
            abi_blob_capnp::blob_delete_response::response::Which::StorageError(()) => Err(BlobDeleteError::StorageError),
        }
    }
}

pub fn blob(binding: impl Into<String>) -> BlobBucket {
    BlobBucket::new(binding)
}

#[derive(Debug, Error)]
pub enum BlobPutError {
    #[error("failed to put blob because of an error in runtime storage implementation")]
    StorageError,
    #[error("failed to read blob because of internal error in fx sdk")]
    InternalSdkError,
}

#[derive(Debug, Error)]
pub enum BlobGetError {
    #[error("blob binding with this name does not exist")]
    BindingNotExists,

    #[error("failed to read blob because of internal error in fx sdk")]
    InternalSdkError,

    #[error("failed to get blob because of an error in runtime storage implementation")]
    StorageError,
}

#[derive(Debug, Error)]
pub enum BlobDeleteError {
    #[error("failed to delete blob because of an error in runtime storage implementation")]
    StorageError,
    #[error("failed to read blob because of internal error in fx sdk")]
    InternalSdkError,
}
