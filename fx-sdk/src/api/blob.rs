use {
    fx_types::{
        capnp,
        abi_blob_capnp,
        abi::{AsyncResourcePollResult, BlobGetResultSerializeResult, BlobDeleteResultSerializeResult},
    },
    thiserror::Error,
    crate::sys::{
        fx_blob_put,
        fx_blob_get,
        fx_blob_delete,
        fx_blob_get_result_poll,
        fx_blob_get_result_serialize,
        fx_blob_delete_result_poll,
        fx_blob_delete_result_serialize,
        fx_bytes_move,
        HostUnitFuture,
    },
};

pub struct BlobBucket {
    binding: String,
}

impl BlobBucket {
    pub fn new(binding: impl Into<String>) -> Self {
        Self { binding: binding.into() }
    }

    pub async fn put(&self, key: String, value: Vec<u8>) {
        HostUnitFuture::new(unsafe { fx_blob_put(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64,
            value.as_ptr() as u64,
            value.len() as u64,
        ) }).await.unwrap();
    }

    pub async fn get(&self, key: String) -> Result<Option<Vec<u8>>, BlobGetError> {
        match BlobGetResponseFuture(unsafe { fx_blob_get(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64
        ) }).await {
            BlobGetResponse::NotFound => Ok(None),
            BlobGetResponse::Ok(v) => Ok(Some(v)),
            BlobGetResponse::BindingNotExists => Err(BlobGetError::BindingNotExists),
            BlobGetResponse::InternalSdkError => Err(BlobGetError::InternalSdkError),
            BlobGetResponse::StorageError => Err(BlobGetError::StorageError),
        }
    }

    pub async fn delete(&self, key: String) -> Result<(), BlobDeleteError> {
        BlobDeleteResponseFuture(unsafe { fx_blob_delete(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64,
        ) }).await
    }
}

pub fn blob(binding: impl Into<String>) -> BlobBucket {
    BlobBucket::new(binding)
}

struct BlobGetResponseFuture(u64);

impl Future for BlobGetResponseFuture {
    type Output = BlobGetResponse;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<AsyncResourcePollResult>::zeroed();
        assert!(unsafe { fx_blob_get_result_poll(self.0, result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<BlobGetResultSerializeResult>::zeroed();
                assert!(unsafe { fx_blob_get_result_serialize(result.resolved_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                let request = resource_reader.get_root::<abi_blob_capnp::blob_get_response::Reader>().unwrap();
                match request.get_response().which().unwrap() {
                    abi_blob_capnp::blob_get_response::response::Which::NotFound(_) => BlobGetResponse::NotFound,
                    abi_blob_capnp::blob_get_response::response::Which::Value(v) => BlobGetResponse::Ok(v.unwrap().to_vec()),
                    abi_blob_capnp::blob_get_response::response::Which::BindingNotExists(_) => BlobGetResponse::BindingNotExists,
                    abi_blob_capnp::blob_get_response::response::Which::BadRequestArgumentOutOfBounds(_)
                    | abi_blob_capnp::blob_get_response::response::Which::BadRequestArgumentFailedToDecode(_)
                    | abi_blob_capnp::blob_get_response::response::Which::BadRequestFailedToAccessMemory(_) => BlobGetResponse::InternalSdkError,
                    abi_blob_capnp::blob_get_response::response::Which::StorageError(()) => BlobGetResponse::StorageError,
                }
            }),
            _ => std::task::Poll::Ready(BlobGetResponse::InternalSdkError),
        }
    }
}

// TODO: convert to Result<Option<Vec<u8>>, BlobGetError>,
enum BlobGetResponse {
    NotFound,
    Ok(Vec<u8>),
    BindingNotExists,
    InternalSdkError,
    StorageError,
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

struct BlobDeleteResponseFuture(u64);

impl Future for BlobDeleteResponseFuture {
    type Output = Result<(), BlobDeleteError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<AsyncResourcePollResult>::zeroed();
        assert!(unsafe { fx_blob_delete_result_poll(self.0, result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<BlobDeleteResultSerializeResult>::zeroed();
                assert!(unsafe { fx_blob_delete_result_serialize(result.resolved_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                let request = resource_reader.get_root::<abi_blob_capnp::blob_delete_response::Reader>().unwrap();
                match request.get_response().which().unwrap() {
                    abi_blob_capnp::blob_delete_response::response::Which::Ok(()) => Ok(()),
                    abi_blob_capnp::blob_delete_response::response::Which::StorageError(()) => Err(BlobDeleteError::StorageError),
                }
            }),
            _ => std::task::Poll::Ready(Err(BlobDeleteError::InternalSdkError)),
        }
    }
}

#[derive(Debug, Error)]
pub enum BlobDeleteError {
    #[error("failed to delete blob because of an error in runtime storage implementation")]
    StorageError,
    #[error("failed to read blob because of internal error in fx sdk")]
    InternalSdkError,
}
