use {
    std::{cell::Cell, rc::Rc},
    futures::future::{BoxFuture, LocalBoxFuture},
    slotmap::{Key, SlotMap},
    send_wrapper::SendWrapper,
    crate::{
        function::instance::FunctionInstance,
        triggers::http::{FetchRequestHeader, HttpBody},
        effects::{
            sql::{SqlRow, SqlQueryError, SqlBatchError, SqlMigrationError},
            blob::BlobGetResponse,
            fetch::FetchResult,
            kv::{KvGetResponse, KvSetError, KvSubscriptionResource},
        },
    },
    super::{
        future::FutureResource,
        serialize::SerializableResource,
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

// TODO: extract into separate resource maps
pub(crate) enum Resource {
    HttpBody(HttpBody),
    SqlMigrationResult(FutureResource<SerializableResource<Result<(), SqlMigrationError>>>),
    SqlQueryResult(FutureResource<SerializableResource<Result<Vec<SqlRow>, SqlQueryError>>>),
    SqlBatchResult(FutureResource<SerializableResource<Result<(), SqlBatchError>>>),
    UnitFuture(BoxFuture<'static, ()>),
    ResourceFuture(SendWrapper<LocalBoxFuture<'static, Box<Resource>>>),
    BlobGetResult(FutureResource<SerializableResource<BlobGetResponse>>),
    FetchResult(FetchResult),
    KvSetResult(FutureResource<SerializableResource<Result<(), KvSetError>>>),
    KvSubscription(KvSubscriptionResource),
}

pub(crate) struct FunctionResources {
    bytes: SlotMap<slotmap::DefaultKey, Vec<u8>>,
    fetch_request_headers: SlotMap<slotmap::DefaultKey, FetchRequestHeader>,
    kv_get_response_futures: SlotMap<slotmap::DefaultKey, BoxFuture<'static, KvGetResponse>>,
    kv_get_responses: SlotMap<slotmap::DefaultKey, KvGetResponse>
}

impl FunctionResources {
    pub(crate) fn new() -> Self {
        Self {
            bytes: SlotMap::new(),
            fetch_request_headers: SlotMap::new(),
            kv_get_response_futures: SlotMap::new(),
            kv_get_responses: SlotMap::new(),
        }
    }

    pub(crate) fn bytes_add(&mut self, bytes: Vec<u8>) -> BytesResourceKey {
        self.bytes.insert(bytes).into()
    }

    pub(crate) fn bytes_get(&self, key: BytesResourceKey) -> Option<&Vec<u8>> {
        self.bytes.get(key.into())
    }

    pub(crate) fn bytes_remove(&mut self, key: BytesResourceKey) -> Option<Vec<u8>> {
        self.bytes.remove(key.into())
    }

    pub(crate) fn fetch_request_header_add(&mut self, header: FetchRequestHeader) -> FetchRequestHeaderResourceKey {
        self.fetch_request_headers.insert(header).into()
    }

    pub(crate) fn fetch_request_header_remove(&mut self, key: FetchRequestHeaderResourceKey) -> Option<FetchRequestHeader> {
        self.fetch_request_headers.remove(key.into())
    }

    pub(crate) fn kv_get_response_futures_add(&mut self, future: BoxFuture<'static, KvGetResponse>) -> KvGetResponseFutureResourceKey {
        self.kv_get_response_futures.insert(future).into()
    }

    pub(crate) fn kv_get_response_futures_get_mut(&mut self, key: KvGetResponseFutureResourceKey) -> Option<&mut BoxFuture<'static, KvGetResponse>> {
        self.kv_get_response_futures.get_mut(key.into())
    }

    pub(crate) fn kv_get_response_futures_remove(&mut self, key: KvGetResponseFutureResourceKey) -> Option<BoxFuture<'static, KvGetResponse>> {
        self.kv_get_response_futures.remove(key.into())
    }

    pub(crate) fn kv_get_response_add(&mut self, response: KvGetResponse) -> KvGetResponseKey {
        self.kv_get_responses.insert(response).into()
    }
}

pub(crate) struct BytesResourceKey {
    id: u64,
}

impl BytesResourceKey {
    pub(crate) fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<slotmap::DefaultKey> for BytesResourceKey {
    fn from(value: slotmap::DefaultKey) -> Self {
        Self { id: value.data().as_ffi() }
    }
}

impl From<u64> for BytesResourceKey {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

impl Into<slotmap::DefaultKey> for BytesResourceKey {
    fn into(self) -> slotmap::DefaultKey {
        slotmap::DefaultKey::from(slotmap::KeyData::from_ffi(self.id))
    }
}

pub(crate) struct FetchRequestHeaderResourceKey {
    id: u64,
}

impl FetchRequestHeaderResourceKey {
    pub(crate) fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<slotmap::DefaultKey> for FetchRequestHeaderResourceKey {
    fn from(value: slotmap::DefaultKey) -> Self {
        Self { id: value.data().as_ffi() }
    }
}

impl From<u64> for FetchRequestHeaderResourceKey {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

impl Into<slotmap::DefaultKey> for FetchRequestHeaderResourceKey {
    fn into(self) -> slotmap::DefaultKey {
        slotmap::DefaultKey::from(slotmap::KeyData::from_ffi(self.id))
    }
}

#[derive(Clone)]
pub(crate) struct KvGetResponseFutureResourceKey(u64);

impl From<slotmap::DefaultKey> for KvGetResponseFutureResourceKey {
    fn from(value: slotmap::DefaultKey) -> Self {
        Self(value.data().as_ffi())
    }
}

impl Into<u64> for KvGetResponseFutureResourceKey {
    fn into(self) -> u64 {
        self.0
    }
}

impl From<u64> for KvGetResponseFutureResourceKey {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl Into<slotmap::DefaultKey> for KvGetResponseFutureResourceKey {
    fn into(self) -> slotmap::DefaultKey {
        slotmap::DefaultKey::from(slotmap::KeyData::from_ffi(self.0))
    }
}

pub(crate) struct KvGetResponseKey(u64);

impl From<slotmap::DefaultKey> for KvGetResponseKey {
    fn from(value: slotmap::DefaultKey) -> Self {
        Self(value.data().as_ffi())
    }
}
