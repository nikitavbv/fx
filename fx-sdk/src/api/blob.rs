use crate::sys::{fx_blob_put, fx_blob_get, FutureHostResource, HostUnitFuture, OwnedResourceId};

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

    pub async fn get(&self, key: String) -> Vec<u8> {
        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_blob_get(
            self.binding.as_ptr() as u64,
            self.binding.len() as u64,
            key.as_ptr() as u64,
            key.len() as u64
        ) });
        FutureHostResource::new(resource_id).await
    }
}

pub fn blob(binding: impl Into<String>) -> BlobBucket {
    BlobBucket::new(binding)
}
