use {
    std::sync::{Arc, Mutex},
    rusqlite::Connection,
};

pub trait KVStorage {
    fn set(&self, key: &[u8], value: &[u8]);
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;
}

#[derive(Clone)]
pub struct SqliteStorage {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        Self::from_connection(Connection::open(path).unwrap())
    }

    pub fn in_memory() -> Self {
        Self::from_connection(Connection::open_in_memory().unwrap())
    }

    fn from_connection(connection: Connection) -> Self {
        connection.execute("create table kv (key blob primary key, value blob)", ()).unwrap();
        Self { connection: Arc::new(Mutex::new(connection)) }
    }
}

impl KVStorage for SqliteStorage {
    fn set(&self, key: &[u8], value: &[u8]) {
        let connection = self.connection.lock().unwrap();
        connection.execute("insert or replace into kv (key, value) values (?1, ?2)", (&key, &value)).unwrap();
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let connection = self.connection.lock().unwrap();
        connection.prepare("select value from kv where key = ?1")
            .unwrap()
            .query_map([(key)], |row| Ok(row.get(0).unwrap()))
            .unwrap()
            .next()
            .map(|v| v.unwrap())
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
    fn set(&self, key: &[u8], value: &[u8]) { self.inner.set(&self.namespaced_key(key), value) }
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> { self.inner.get(&self.namespaced_key(key)) }
}

pub struct EmptyStorage;

impl KVStorage for EmptyStorage {
    fn get(&self, _key: &[u8]) -> Option<Vec<u8>> { None }
    fn set(&self, _key: &[u8], _value: &[u8]) {}
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
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.inner.get(key)
    }

    fn set(&self, key: &[u8], value: &[u8]) {
        self.inner.set(key, value)
    }
}

pub trait WithKey {
    fn with_key(self, key: &[u8], value: &[u8]) -> Self;
}

impl<S: KVStorage> WithKey for S {
    fn with_key(self, key: &[u8], value: &[u8]) -> Self {
        self.set(key, value);
        self
    }
}
