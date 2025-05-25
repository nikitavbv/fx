use {
    std::{sync::{Arc, Mutex}, path::{self, PathBuf}, fs, io, future},
    rusqlite::Connection,
    futures::{stream::{self, BoxStream}, StreamExt},
    tokio::sync::mpsc,
    notify::Watcher,
    crate::error::FxCloudError,
};

pub trait KVStorage {
    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxCloudError>;
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError>;

    fn watch(&self) -> BoxStream<KeyUpdate> {
        stream::empty().boxed()
    }
}

pub struct KeyUpdate {
    pub key: Vec<u8>,
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
        connection.execute("create table if not exists kv (key blob primary key, value blob)", ())
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

#[derive(Clone)]
pub struct FsStorage {
    path: PathBuf,
    watchers: Arc<Mutex<Vec<Box<dyn notify::Watcher + Send>>>>,
}

impl FsStorage {
    pub fn new(path: PathBuf) -> Self {
        fs::create_dir_all(&path).unwrap();
        Self {
            path,
            watchers: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl KVStorage for FsStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError> {
        match fs::read(self.path.join(&String::from_utf8(key.to_vec()).unwrap())) {
            Ok(v) => Ok(Some(v)),
            Err(err) => {
                if err.kind() == io::ErrorKind::NotFound {
                    return Ok(None)
                } else {
                    return Err(FxCloudError::StorageInternalError { reason: format!("failed to read file: {err:?}") })
                }
            }
        }
    }

    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxCloudError> {
        let path = self.path.join(&String::from_utf8(key.to_vec()).unwrap());
        fs::write(path, value).unwrap();
        Ok(())
    }

    fn watch(&self) -> BoxStream<KeyUpdate> {
        let base_path = path::absolute(PathBuf::from(self.path.clone())).unwrap();
        println!("running watch on {:?}", base_path);
        let (tx, mut rx) = mpsc::channel(1024);
        let event_fn = {
            let base_path = base_path.clone();

            move |res: notify::Result<notify::Event>| {
                let res = res.unwrap();
                println!("fs notify change kind: {:?}", res.kind);
                for changed_path in res.paths {
                    let relative = pathdiff::diff_paths(changed_path, &base_path).unwrap();
                    tx.blocking_send(KeyUpdate {
                        key: relative.to_str().unwrap().as_bytes().to_vec(),
                    }).unwrap();
                }
            }
        };
        let mut watcher = notify::recommended_watcher(event_fn).unwrap();
        watcher.watch(&base_path, notify::RecursiveMode::Recursive).unwrap();
        self.watchers.lock().unwrap().push(Box::new(watcher));

        async_stream::stream! {
            while let Some(item) = rx.recv().await {
                yield item;
            }
            println!("stopping here");
        }.boxed()
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

pub struct SuffixStorage<T> {
    suffix: Vec<u8>,
    inner: T,
}

impl<T> SuffixStorage<T> {
    pub fn new(suffix: impl Into<Vec<u8>>, inner: T) -> Self {
        Self {
            suffix: suffix.into(),
            inner,
        }
    }

    fn suffixed_key(&self, key: &[u8]) -> Vec<u8> {
        let mut suffixed_key = Vec::with_capacity(key.len() + self.suffix.len());
        suffixed_key.extend_from_slice(key);
        suffixed_key.extend_from_slice(&self.suffix);
        suffixed_key
    }
}

impl<T: KVStorage> KVStorage for SuffixStorage<T> {
    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxCloudError> { self.inner.set(&self.suffixed_key(key), value) }
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError> { self.inner.get(&self.suffixed_key(key)) }

    fn watch(&self) -> BoxStream<KeyUpdate> {
        let suffix = self.suffix.clone();
        let suffix_len = suffix.len();

        self.inner
            .watch()
            .filter(move |v| future::ready(v.key.ends_with(&suffix)))
            .map(move |v| KeyUpdate {
                key: v.key[0..v.key.len() - suffix_len].to_vec(),
            })
            .boxed()
    }
}

impl<T: Clone> Clone for SuffixStorage<T> {
    fn clone(&self) -> Self {
        Self {
            suffix: self.suffix.clone(),
            inner: self.inner.clone(),
        }
    }
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

    fn watch(&self) -> BoxStream<KeyUpdate> {
        self.inner.watch()
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
