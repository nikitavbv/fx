use {
    std::{sync::{Arc, Mutex}, path::{self, PathBuf}, fs, io, future},
    tracing::{info, error},
    rusqlite::Connection,
    futures::{stream::{self, BoxStream, empty as empty_stream}, StreamExt},
    tokio::sync::mpsc,
    notify::Watcher,
    fx_runtime::{
        error::{FxRuntimeError, KVWatchError},
        kv::KVStorage,
    },
};

#[derive(Clone)]
pub struct SqliteKV {
    // todo: make connection thread local?
    connection: Arc<Mutex<Connection>>,
}

impl SqliteKV {
    #[allow(dead_code)]
    pub fn new(path: impl AsRef<std::path::Path>) -> Result<Self, FxRuntimeError> {
        Self::from_connection(
            Connection::open(path)
                .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to open sqlite database: {err:?}") })?
        )
    }

    pub fn in_memory() -> Result<Self, FxRuntimeError> {
        Self::from_connection(
            Connection::open_in_memory()
                .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to open in memory sqlite: {err:?}") })?
        )
    }

    fn from_connection(connection: Connection) -> Result<Self, FxRuntimeError> {
        connection.execute("create table if not exists kv (key blob primary key, value blob)", ())
            .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to create kv table: {err:?}") })?;
        Ok(Self { connection: Arc::new(Mutex::new(connection)) })
    }
}

impl KVStorage for SqliteKV {
    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), FxRuntimeError> {
        let connection = self.connection.lock()
            .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to acquire sqlite connection: {err:?}") })?;
        connection.execute("insert or replace into kv (key, value) values (?1, ?2)", (&key, &value))
            .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to execute sqlite query: {err:?}") })
            .map(|_| ())
    }

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, FxRuntimeError> {
        let connection = self.connection.lock()
            .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to acquire sqlite connection: {err:?}") })?;
        let mut stmt = connection.prepare("select value from kv where key = ?1")
            .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to prepare sqlite query: {err:?}") })?;
        let mut rows = stmt.query([(key)])
            .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to map sqlite result to value: {err:?}") })?;

        let res = rows.next()
            .map_err(|err| FxRuntimeError::StorageInternalError { reason: format!("failed to read row from sqlite result: {err:?}") })?
            .map(|v| v.get(0));

        match res {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(err)) => Err(FxRuntimeError::StorageInternalError { reason: format!("failed to decode sqlite result: {err:?}") }),
            None => Ok(None)
        }
    }

    fn list(&self) -> Result<Vec<Vec<u8>>, FxRuntimeError> {
        unimplemented!()
    }
}
