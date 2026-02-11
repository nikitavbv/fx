use {
    fx_types::{capnp, abi_blob_capnp},
    crate::sys::{
        fx_blob_put,
        fx_blob_get,
        fx_blob_delete,
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
        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_blob_put(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64,
            value.as_ptr() as u64,
            value.len() as u64,
        ) });
        HostUnitFuture::new(resource_id).await;
    }

    pub async fn get(&self, key: String) -> Option<Vec<u8>> {
        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_blob_get(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64
        ) });
        match FutureHostResource::<BlobGetResponse>::new(resource_id).await {
            BlobGetResponse::NotFound => None,
            BlobGetResponse::Ok(v) => Some(v),
        }
    }

    pub async fn delete(&self, key: String) {
        HostUnitFuture::new(OwnedResourceId::from_ffi(unsafe { fx_blob_delete(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64,
        ) })).await;
    }
}

pub fn blob(binding: impl Into<String>) -> BlobBucket {
    BlobBucket::new(binding)
}

enum BlobGetResponse {
    NotFound,
    Ok(Vec<u8>),
}

impl DeserializeHostResource for BlobGetResponse {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<abi_blob_capnp::blob_get_response::Reader>().unwrap();
        match request.get_response().which().unwrap() {
            abi_blob_capnp::blob_get_response::response::Which::NotFound(_) => BlobGetResponse::NotFound,
            abi_blob_capnp::blob_get_response::response::Which::Value(v) => BlobGetResponse::Ok(v.unwrap().to_vec()),
        }
    }
}
