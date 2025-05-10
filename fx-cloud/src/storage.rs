use {
    std::sync::{Arc, Mutex},
    rusqlite::Connection,
    crate::error::FxCloudError,
};

pub trait KVStorage {
    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxCloudError>;
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError>;
}

#[derive(Clone)]
pub struct SqliteStorage {
    // todo: make connection thread local?
    connection: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    #[allow(dead_code)]
    pub fn new(path: impl AsRef<std::path::Path>) -> Result<Self, FxCloudError> {
        Self::from_connection(
            Connection::open(path)
                .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to open sqlite database: {err:?}") })?
        )
    }

    pub fn in_memory() -> Result<Self, FxCloudError> {
        Self::from_connection(
            Connection::open_in_memory()
                .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to open in memory sqlite: {err:?}") })?
        )
    }

    fn from_connection(connection: Connection) -> Result<Self, FxCloudError> {
        connection.execute("create table kv (key blob primary key, value blob)", ())
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to create kv table: {err:?}") })?;
        Ok(Self { connection: Arc::new(Mutex::new(connection)) })
    }
}

impl KVStorage for SqliteStorage {
    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxCloudError> {
        let connection = self.connection.lock()
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to acquire sqlite connection: {err:?}") })?;
        connection.execute("insert or replace into kv (key, value) values (?1, ?2)", (&key, &value))
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to execute sqlite query: {err:?}") })
            .map(|_| ())
    }

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError> {
        let connection = self.connection.lock()
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to acquire sqlite connection: {err:?}") })?;
        let mut stmt = connection.prepare("select value from kv where key = ?1")
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to prepare sqlite query: {err:?}") })?;
        let mut rows = stmt.query([(key)])
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to map sqlite result to value: {err:?}") })?;

        let res = rows.next()
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to read row from sqlite result: {err:?}") })?
            .map(|v| v.get(0));

        match res {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(err)) => Err(FxCloudError::StorageInternalError { reason: format!("failed to decode sqlite result: {err:?}") }),
            None => Ok(None)
        }
    }
}

pub struct NamespacedStorage<T> {
    namespace: Vec<u8>,
    inner: T,
}

impl<T> NamespacedStorage<T> {
    pub fn new(namespace: impl Into<Vec<u8>>, inner: T) -> Self {
        Self {
            namespace: namespace.into(),
            inner,
        }
    }

    fn namespaced_key(&self, key: &[u8]) -> Vec<u8> {
        let mut namespaced_key = Vec::with_capacity(self.namespace.len() + key.len());
        namespaced_key.extend_from_slice(&self.namespace);
        namespaced_key.extend_from_slice(key);
        namespaced_key
    }
}

impl<T: KVStorage> KVStorage for NamespacedStorage<T> {
    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxCloudError> { self.inner.set(&self.namespaced_key(key), value) }
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError> { self.inner.get(&self.namespaced_key(key)) }
}

pub struct EmptyStorage;

impl KVStorage for EmptyStorage {
    fn get(&self, _key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError> { Ok(None) }
    fn set(&self, _key: &[u8], _value: &[u8]) -> Result<(), FxCloudError> { Ok(()) }
}

#[derive(Clone)]
pub struct BoxedStorage {
    inner: Arc<Box<dyn KVStorage + Send + Sync>>,
}

impl BoxedStorage {
    pub fn new<T: KVStorage + Send + Sync + 'static>(inner: T) -> Self {
        Self {
            inner: Arc::new(Box::new(inner)),
        }
    }
}

impl KVStorage for BoxedStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError> {
        self.inner.get(key)
    }

    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxCloudError> {
        self.inner.set(key, value)
    }
}

pub trait WithKey: Sized {
    fn with_key(self, key: &[u8], value: &[u8]) -> Result<Self, FxCloudError>;
}

impl<S: KVStorage> WithKey for S {
    fn with_key(self, key: &[u8], value: &[u8]) -> Result<Self, FxCloudError> {
        self.set(key, value)?;
        Ok(self)
    }
}
