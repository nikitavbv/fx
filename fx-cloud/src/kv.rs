use {
    std::{sync::{Arc, Mutex}, path::{self, PathBuf}, fs, io, future},
    tracing::{info, error},
    rusqlite::Connection,
    futures::{stream::{self, BoxStream, empty as empty_stream}, StreamExt},
    tokio::sync::mpsc,
    notify::Watcher,
    crate::error::{FxCloudError, KVWatchError},
};

pub trait KVStorage {
    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxCloudError>;
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError>;
    fn list(&self) -> Result<Vec<Vec<u8>>, FxCloudError>;

    fn watch(&self) -> BoxStream<Result<KeyUpdate, KVWatchError>> {
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

    fn list(&self) -> Result<Vec<Vec<u8>>, FxCloudError> {
        unimplemented!()
    }
}

#[derive(Clone)]
pub struct FsStorage {
    path: PathBuf,
    watchers: Arc<Mutex<Vec<Box<dyn notify::Watcher + Send>>>>,
}

impl FsStorage {
    pub fn new(path: PathBuf) -> Result<Self, FxCloudError> {
        fs::create_dir_all(&path)
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to create directory for FsStorage: {err:?}") })?;
        Ok(Self {
            path,
            watchers: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn path_for_key(&self, key: &[u8]) -> Result<PathBuf, FxCloudError> {
        Ok(self.path.join(
            &String::from_utf8(key.to_vec())
                .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to decode key as string: {err:?}") })?
        ))
    }
}

impl KVStorage for FsStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxCloudError> {
        match fs::read(self.path_for_key(key)?) {
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
        let path = self.path_for_key(key)?;
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to create parent directory for FsStorage: {err:?}") })?;
            }
        }
        fs::write(path, value)
            .map_err(|err| FxCloudError::StorageInternalError { reason: format!("failed to write filed: {err:?}") })?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<Vec<u8>>, FxCloudError> {
        unimplemented!()
    }

    fn watch(&self) -> BoxStream<Result<KeyUpdate, KVWatchError>> {
        let base_path = match path::absolute(PathBuf::from(self.path.clone())) {
            Ok(v) => v,
            Err(err) => {
                let err = KVWatchError::FailedToInit { reason: format!("failed to get absolute path for path: {:?}, error: {err:?}", self.path) };
                return stream::once(async move { Err(err) }).boxed();
            }
        };

        info!("running watch on {:?}", base_path);

        let (tx, mut rx) = mpsc::channel(1024);
        let event_fn = {
            let base_path = base_path.clone();

            move |res: notify::Result<notify::Event>| {
                let res = match res {
                    Ok(v) => v,
                    Err(err) => {
                        if let Err(err) = tx.blocking_send(Err(KVWatchError::EventHandling {
                            reason: format!("received notify error: {err:?}"),
                        })) {
                            error!("failed to send watch event when handling notify error: {err:?}");
                        }
                        return;
                    }
                };

                match res.kind {
                    notify::EventKind::Access(_)
                    | notify::EventKind::Remove(_) => {},
                    _other => {
                        for changed_path in res.paths {
                            let relative = pathdiff::diff_paths(&changed_path, &base_path);
                            let result = tx.blocking_send({
                                if let Some(relative) = relative {
                                    match relative.to_str() {
                                        Some(v) => Ok(KeyUpdate {
                                            key: v.as_bytes().to_vec(),
                                        }),
                                        None => Err(KVWatchError::EventHandling {
                                            reason: format!("failed to convert pathdiff to str for changed path: {changed_path:?}"),
                                        }),
                                    }
                                } else {
                                    Err(KVWatchError::EventHandling {
                                        reason: format!("failed to pathdiff for changed path: {changed_path:?}"),
                                    })
                                }
                            });
                            if let Err(err) = result {
                                error!("failed to send watch event: {err:?}");
                            }
                        }
                    }
                }
            }
        };
        let mut watcher = match notify::recommended_watcher(event_fn) {
            Ok(v) => v,
            Err(err) => {
                let err = KVWatchError::FailedToInit { reason: format!("failed to create watcher: {err:?}") };
                return stream::once(async move { Err(err) }).boxed();
            },
        };
        if let Err(err) = watcher.watch(&base_path, notify::RecursiveMode::Recursive) {
            let err = KVWatchError::FailedToInit { reason: format!("failed to watch path: {err:?}") };
            return stream::once(async move { Err(err) }).boxed();
        }

        {
            let mut watchers = match self.watchers.lock() {
                Ok(v) => v,
                Err(err) => {
                    let err = KVWatchError::FailedToInit { reason: format!("failed to lock watchers: {err:?}") };
                    return stream::once(async move { Err(err) }).boxed();
                }
            };
            watchers.push(Box::new(watcher));
        }

        async_stream::stream! {
            while let Some(item) = rx.recv().await {
                yield item;
            }
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
    fn list(&self) -> Result<Vec<Vec<u8>>, FxCloudError> { unimplemented!() }
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
    fn list(&self) -> Result<Vec<Vec<u8>>, FxCloudError> {
        unimplemented!()
    }

    fn watch(&self) -> BoxStream<Result<KeyUpdate, KVWatchError>> {
        let suffix = self.suffix.clone();
        let suffix_len = suffix.len();

        self.inner
            .watch()
            .filter(move |v| future::ready(match v {
                Ok(v) => v.key.ends_with(&suffix),
                Err(_err) => true,
            }))
            .map(move |v| v.map(|v| KeyUpdate {
                key: v.key[0..v.key.len() - suffix_len].to_vec(),
            }))
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
    fn list(&self) -> Result<Vec<Vec<u8>>, FxCloudError> { Ok(Vec::new()) }
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

    fn list(&self) -> Result<Vec<Vec<u8>>, FxCloudError> {
        self.inner.list()
    }

    fn watch(&self) -> BoxStream<Result<KeyUpdate, KVWatchError>> {
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
