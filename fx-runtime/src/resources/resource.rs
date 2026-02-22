use {
    std::{cell::Cell, rc::Rc},
    futures::future::BoxFuture,
    slotmap::Key,
    crate::{
        function::{instance::FunctionInstance, abi::{capnp, abi_function_resources_capnp}},
        triggers::http::{FunctionResponse, FunctionResponseInner, FunctionHttpResponse, FetchRequestHeader, FetchRequestBody},
        effects::{
            sql::{SqlQueryResult, SqlMigrationResult},
            blob::BlobGetResponse,
            fetch::FetchResult,
        },
    },
    super::{
        future::FutureResource,
        serialize::{SerializedFunctionResource, SerializableResource},
    },
};

/// Function resource handle that is owned by host.
/// Cleans up function memory if dropped before being consumed
pub struct OwnedFunctionResourceId(Cell<Option<(Rc<FunctionInstance>, FunctionResourceId)>>);

impl OwnedFunctionResourceId {
    pub fn new(function_instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self(Cell::new(Some((function_instance, resource_id))))
    }

    pub fn consume(self) -> (Rc<FunctionInstance>, FunctionResourceId) {
        self.0.replace(None).unwrap()
    }
}

impl Drop for OwnedFunctionResourceId {
    fn drop(&mut self) {
        if let Some((function_instance, resource_id)) = self.0.replace(None) {
            tokio::task::spawn_local(async move {
                function_instance.resource_drop(&resource_id).await;
            });
        }
    }
}

pub(crate) struct ResourceId {
    id: u64,
}

impl ResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<slotmap::DefaultKey> for ResourceId {
    fn from(value: slotmap::DefaultKey) -> Self {
        Self::new(value.data().as_ffi())
    }
}

impl Into<slotmap::DefaultKey> for &ResourceId {
    fn into(self) -> slotmap::DefaultKey {
        slotmap::DefaultKey::from(slotmap::KeyData::from_ffi(self.id))
    }
}

impl From<u64> for ResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionResourceId {
    id: u64,
}

impl FunctionResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<u64> for FunctionResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

pub(crate) enum Resource {
    FetchRequest(SerializableResource<FetchRequestHeader>),
    RequestBody(FetchRequestBody),
    SqlQueryResult(FutureResource<SerializableResource<SqlQueryResult>>),
    SqlMigrationResult(FutureResource<SerializableResource<SqlMigrationResult>>),
    UnitFuture(BoxFuture<'static, ()>),
    BlobGetResult(FutureResource<SerializableResource<BlobGetResponse>>),
    FetchResult(FutureResource<SerializableResource<FetchResult>>),
}
