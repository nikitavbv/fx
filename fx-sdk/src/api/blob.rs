use {
    fx_types::{capnp, abi_blob_capnp, abi::AsyncResourcePollResult},
    thiserror::Error,
    crate::sys::{
        fx_blob_put,
        fx_blob_get,
        fx_blob_delete,
        fx_blob_get_result_poll,
        FutureHostResource,
        HostUnitFuture,
        OwnedResourceId,
        DeserializeHostResource,
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
        ) }).await;
    }

    pub async fn get(&self, key: String) -> Result<Option<Vec<u8>>, BlobGetError> {
        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_blob_get(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64
        ) });
        match FutureHostResource::<BlobGetResponse>::new(resource_id).await {
            BlobGetResponse::NotFound => Ok(None),
            BlobGetResponse::Ok(v) => Ok(Some(v)),
            BlobGetResponse::BindingNotExists => Err(BlobGetError::BindingNotExists),
            BlobGetResponse::InternalSdkError => Err(BlobGetError::InternalSdkError),
        }
    }

    pub async fn delete(&self, key: String) {
        HostUnitFuture::new(unsafe { fx_blob_delete(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64,
        ) }).await;
    }
}

pub fn blob(binding: impl Into<String>) -> BlobBucket {
    BlobBucket::new(binding)
}

struct BlobGetResponseFuture(u64);

impl Future for BlobGetResponseFuture {
    type Output = BlobGetResponse;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<AsyncResourcePollResult>::zeroed();
        assert!(unsafe { fx_blob_get_result_poll(self.0, result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };

        todo!()
    }
}

enum BlobGetResponse {
    NotFound,
    Ok(Vec<u8>),
    BindingNotExists,
    InternalSdkError,
}

impl DeserializeHostResource for BlobGetResponse {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<abi_blob_capnp::blob_get_response::Reader>().unwrap();
        match request.get_response().which().unwrap() {
            abi_blob_capnp::blob_get_response::response::Which::NotFound(_) => BlobGetResponse::NotFound,
            abi_blob_capnp::blob_get_response::response::Which::Value(v) => BlobGetResponse::Ok(v.unwrap().to_vec()),
            abi_blob_capnp::blob_get_response::response::Which::BindingNotExists(_) => BlobGetResponse::BindingNotExists,
            abi_blob_capnp::blob_get_response::response::Which::BadRequestArgumentOutOfBounds(_)
            | abi_blob_capnp::blob_get_response::response::Which::BadRequestArgumentFailedToDecode(_)
            | abi_blob_capnp::blob_get_response::response::Which::BadRequestFailedToAccessMemory(_) => BlobGetResponse::InternalSdkError,
        }
    }
}

#[derive(Debug, Error)]
pub enum BlobGetError {
    #[error("blob binding with this name does not exist")]
    BindingNotExists,

    #[error("failed to read sdk because of internal error in fx sdk")]
    InternalSdkError,
}
