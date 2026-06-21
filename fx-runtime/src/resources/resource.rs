use {
    std::{cell::Cell, rc::Rc, marker::PhantomData},
    futures::future::{BoxFuture, LocalBoxFuture},
    slotmap::{Key, SlotMap},
    send_wrapper::SendWrapper,
    hyper::body::Bytes,
    crate::{
        function::instance::FunctionInstance,
        triggers::http::{FetchRequestHeader, HttpBody},
        effects::{
            sql::{SqlRow, SqlQueryError, SqlBatchError, SqlMigrationError},
            blob::BlobGetResponse,
            fetch::{FetchResult, HttpStreamError},
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
    KvSubscription(KvSubscriptionResource),
}

#[derive(Default)]
pub(crate) struct FunctionResources {
    pub(crate) bytes: ResourceTable<BytesResourceKey, Vec<u8>>,
    pub(crate) fetch_request_headers: ResourceTable<FetchRequestHeaderResourceKey, FetchRequestHeader>,
    pub(crate) kv_get_response_futures: ResourceTable<KvGetResponseFutureResourceKey, BoxFuture<'static, KvGetResponse>>,
    pub(crate) kv_get_responses: ResourceTable<KvGetResponseKey, KvGetResponse>,
    pub(crate) kv_set_response_futures: ResourceTable<KvSetResponseFutureResourceKey, BoxFuture<'static, Result<(), KvSetError>>>,
    pub(crate) kv_set_responses: ResourceTable<KvSetResponseKey, Result<(), KvSetError>>,
    pub(crate) unit_futures: ResourceTable<UnitFutureResourceKey, BoxFuture<'static, ()>>,
    pub(crate) sql_query_result_futures: ResourceTable<SqlQueryResultFutureResourceKey, BoxFuture<'static, Result<Vec<SqlRow>, SqlQueryError>>>,
    pub(crate) sql_query_results: ResourceTable<SqlQueryResultResourceKey, Result<Vec<SqlRow>, SqlQueryError>>,
    pub(crate) sql_batch_result_futures: ResourceTable<SqlBatchResultFutureResourceKey, BoxFuture<'static, Result<(), SqlBatchError>>>,
    pub(crate) sql_batch_results: ResourceTable<SqlBatchResultResourceKey, Result<(), SqlBatchError>>,
    pub(crate) sql_migration_result_futures: ResourceTable<SqlMigrationResultFutureResourceKey, BoxFuture<'static, Result<(), SqlMigrationError>>>,
    pub(crate) sql_migration_results: ResourceTable<SqlMigrationResultResourceKey, Result<(), SqlMigrationError>>,
    pub(crate) fetch_result_futures: ResourceTable<FetchResultFutureResourceKey, SendWrapper<LocalBoxFuture<'static, FetchResult>>>,
    pub(crate) fetch_results: ResourceTable<FetchResultResourceKey, FetchResult>,
    pub(crate) http_bodies: ResourceTable<HttpBodyResourceKey, HttpBody>,
    pub(crate) http_frames: ResourceTable<HttpFrameResourceKey, Option<Result<Bytes, HttpStreamError>>>,
    pub(crate) blob_get_response_futures: ResourceTable<BlobGetResponseFutureResourceKey, BoxFuture<'static, BlobGetResponse>>,
    pub(crate) blob_get_responses: ResourceTable<BlobGetResponseResourceKey, BlobGetResponse>,
}

impl FunctionResources {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

pub(crate) struct ResourceTable<K, V> {
    map: SlotMap<slotmap::DefaultKey, V>,
    _key: PhantomData<K>,
}

impl<K, V> ResourceTable<K, V> {
    pub(crate) fn new() -> Self {
        Self {
            map: SlotMap::new(),
            _key: PhantomData,
        }
    }

    pub fn insert(&mut self, value: V) -> K where K: From<slotmap::DefaultKey> {
        self.map.insert(value).into()
    }

    pub fn get(&self, key: K) -> Option<&V> where K: Into<slotmap::DefaultKey> {
        self.map.get(key.into())
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> where K: Into<slotmap::DefaultKey> {
        self.map.get_mut(key.into())
    }

    pub fn remove(&mut self, key: K) -> Option<V> where K: Into<slotmap::DefaultKey> {
        self.map.remove(key.into())
    }
}

impl<K, V> Default for ResourceTable<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

macro_rules! key {
    ($(#[$meta:meta])* $vis:vis struct $name:ident) => {
        $(#[$meta])*
        #[derive(Clone)]
        $vis struct $name(u64);

        impl ::core::convert::From<::slotmap::DefaultKey> for $name {
            fn from(value: ::slotmap::DefaultKey) -> Self {
                Self(value.data().as_ffi())
            }
        }

        impl ::core::convert::From<$name> for u64 {
            fn from(value: $name) -> u64 {
                value.0
            }
        }

        impl ::core::convert::From<&$name> for u64 {
            fn from(value: &$name) -> u64 {
                value.0
            }
        }

        impl ::core::convert::From<u64> for $name {
            fn from(id: u64) -> Self {
                Self(id)
            }
        }

        impl ::core::convert::From<$name> for ::slotmap::DefaultKey {
            fn from(value: $name) -> ::slotmap::DefaultKey {
                ::slotmap::DefaultKey::from(::slotmap::KeyData::from_ffi(value.0))
            }
        }
    };
}

key!(pub(crate) struct BytesResourceKey);
key!(pub(crate) struct KvGetResponseFutureResourceKey);
key!(pub(crate) struct KvGetResponseKey);
key!(pub(crate) struct FetchRequestHeaderResourceKey);
key!(pub(crate) struct KvSetResponseFutureResourceKey);
key!(pub(crate) struct KvSetResponseKey);
key!(pub(crate) struct UnitFutureResourceKey);
key!(pub(crate) struct SqlQueryResultFutureResourceKey);
key!(pub(crate) struct SqlQueryResultResourceKey);
key!(pub(crate) struct SqlBatchResultFutureResourceKey);
key!(pub(crate) struct SqlBatchResultResourceKey);
key!(pub(crate) struct SqlMigrationResultFutureResourceKey);
key!(pub(crate) struct SqlMigrationResultResourceKey);
key!(pub(crate) struct FetchResultFutureResourceKey);
key!(pub(crate) struct FetchResultResourceKey);
key!(pub(crate) struct HttpBodyResourceKey);
key!(pub(crate) struct HttpFrameResourceKey);
key!(pub(crate) struct BlobGetResponseFutureResourceKey);
key!(pub(crate) struct BlobGetResponseResourceKey);
