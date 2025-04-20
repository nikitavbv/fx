use {
    std::sync::{Arc, Mutex},
    sqlite::{Connection, State},
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
        let connection = sqlite::open(path).unwrap();
        connection.execute("create table kv (key blob primary key, value blob)").unwrap();
        Self { connection: Arc::new(Mutex::new(connection)) }
    }
}

impl KVStorage for SqliteStorage {
    fn set(&self, key: &[u8], value: &[u8]) {
        let connection = self.connection.lock().unwrap();
        let mut stmt = connection.prepare("insert or replace into kv (key, value) values (:key, :value)").unwrap();
        stmt.bind(&[(":key", key), (":value", value)][..]).unwrap();
        while stmt.next().unwrap() != State::Done {}
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let connection = self.connection.lock().unwrap();
        let mut stmt = connection.prepare("select value from kv where key = :key").unwrap();
        stmt.bind((":key", key)).unwrap();

        match stmt.next().unwrap() {
            State::Done => None,
            State::Row => Some(stmt.read("value").unwrap()),
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
