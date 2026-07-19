use {
    std::path::{PathBuf, Path},
    tracing::error,
    thiserror::Error,
    rusqlite::OptionalExtension,
    tokio::{sync::oneshot, fs},
};

pub(crate) enum BlobMessage {
    Put {
        bucket: String,
        key: Vec<u8>,
        value: Vec<u8>,
        result: oneshot::Sender<Result<(), PutError>>,
    },
    Get {
        bucket: String,
        key: Vec<u8>,
        result: oneshot::Sender<Result<Option<Vec<u8>>, GetError>>,
    },
    Delete {
        bucket: String,
        key: Vec<u8>,
        result: oneshot::Sender<Result<(), DeleteError>>,
    },
}

#[derive(Debug, Error)]
pub(crate) enum PutError {
    #[error("failed to put object because of unexpected error in blob storage implementation")]
    BlobStorageError,
}

#[derive(Debug, Error)]
pub(crate) enum GetError {
    #[error("failed to get object because of unexpected error in blob storage implementation")]
    BlobStorageError,
}

#[derive(Debug, Error)]
pub(crate) enum DeleteError {
    #[error("failed to delete object because of unexpected error in blob storage implementation")]
    BlobStorageError,
}

pub(crate) fn run_blob_task(blob_path: PathBuf, blob_rx: flume::Receiver<BlobMessage>) {
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let tokio_runtime = match tokio_runtime {
        Ok(v) => v,
        Err(err) => {
            error!("failed to create tokio runtime for blob task: {err:?}. Stopping blob task.");
            return;
        }
    };
    let local_set = tokio::task::LocalSet::new();

    tokio_runtime.block_on(local_set.run_until(async {
        let data_path = blob_path.join("data");
        match tokio::fs::try_exists(&data_path).await {
            Ok(false) => if let Err(err) = fs::create_dir_all(&data_path).await {
                error!("failed to create data directory for blob storage: {err:?}");
            },
            Ok(true) => {},
            Err(err) => error!("failed to check if data directory for blob storage exists: {err:?}"),
        }

        let index_db = match IndexDb::new(blob_path.join("index.sqlite")) {
            Ok(v) => v,
            Err(()) => {
                error!("failed to create index database. Stopping blob task.");
                return;
            },
        };

        while let Ok(msg) = blob_rx.recv_async().await {
            match msg {
                BlobMessage::Put { bucket, key, value, result } => {
                    let key_hash = hash_key_for_object(bucket.as_str(), &key);
                    if index_db.put_object(&bucket, &key, &key_hash, value.len() as u64).is_err() {
                        let _ = result.send(Err(PutError::BlobStorageError));
                        continue;
                    }

                    let _ = result.send(
                        tokio::fs::write(key_path(&data_path, key_hash.as_str()).await.as_path(), value).await
                            .map_err(|err| {
                                error!("failed to write object to fs: {err:?}");
                                PutError::BlobStorageError
                            })
                    );
                },
                BlobMessage::Get { bucket, key, result } => {
                    let key_hash = match index_db.get_object(&bucket, &key) {
                        Ok(Some(v)) => v,
                        Ok(None) => {
                            let _ = result.send(Ok(None));
                            continue;
                        },
                        Err(()) => {
                            let _ = result.send(Err(GetError::BlobStorageError));
                            continue;
                        },
                    };

                    let value = tokio::fs::read(key_path(&data_path, key_hash.as_str()).await.as_path()).await
                        .map_err(|err| {
                            error!("failed to read object from fs: {err:?}");
                            GetError::BlobStorageError
                        })
                        .map(Some);

                    let _ = result.send(value);
                },
                BlobMessage::Delete { bucket, key, result: result_sender } => {
                    let key_hash = match index_db.delete_object(&bucket, &key) {
                        Ok(Some(v)) => v,
                        Ok(None) => {
                            let _ = result_sender.send(Ok(()));
                            continue;
                        },
                        Err(()) => {
                            let _ = result_sender.send(Err(DeleteError::BlobStorageError));
                            continue;
                        },
                    };

                    let result = tokio::fs::remove_file(key_path(&data_path, key_hash.as_str()).await.as_path()).await
                        .map_err(|err| {
                            error!("failed to remove object file: {err:?}");
                            DeleteError::BlobStorageError
                        });

                    // error here can be ignored because it means request has been cancelled
                    let _ = result_sender.send(result);
                },
            }
        }
    }));
}

fn hash_key_for_object(bucket_name: &str, key: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(bucket_name.len() as u32).to_be_bytes());
    hasher.update(bucket_name.as_bytes());
    hasher.update(key);
    hasher.finalize().to_hex().to_string()
}

async fn key_path(data_path: &Path, key: &str) -> PathBuf {
    let dir_path = data_path.join(&key[0..2]).join(&key[2..4]);

    match tokio::fs::try_exists(&dir_path).await {
        Ok(false) => {
            if let Err(err) = fs::create_dir_all(&dir_path).await {
                error!("failed to create directory for key path: {err:?}");
            }
        },
        Ok(true) => {},
        Err(err) => error!("failed to check if key path directory exists: {err:?}"),
    };

    dir_path.join(&key[4..])
}

struct IndexDb {
    connection: rusqlite::Connection,
}

impl IndexDb {
    fn new(path: PathBuf) -> Result<Self, ()> {
        let mut connection = rusqlite::Connection::open(path)
            .map_err(|err| error!("failed to open connection to index database for blob: {err:?}"))?;
        connection.pragma_update(None, "journal_mode", "WAL")
            .map_err(|err| error!("failed to update pragma for \"journal_mode\" for index db connection: {err:?}"))?;
        connection.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|err| error!("failed to update pragma for \"synchronous\" for index db connection: {err:?}"))?;

        rusqlite_migration::Migrations::new(vec![
            rusqlite_migration::M::up(r#"
                create table buckets (
                    id integer primary key,
                    name text not null unique
                );

                create table objects (
                    bucket_id integer not null references buckets (id),
                    key blob not null,
                    key_hash text not null,
                    size integer not null,
                    primary key (bucket_id, key)
                );
            "#),
        ])
            .to_latest(&mut connection)
            .map_err(|err| error!("failed to run migrations for index database: {err:?}"))?;

        Ok(Self {
            connection,
        })
    }

    fn get_or_create_bucket(&self, name: &str) -> Result<i64, ()> {
        self.connection.execute(
            "insert or ignore into buckets (name) values (?1)",
            rusqlite::params![name],
        ).map_err(|err| error!("failed to create bucket if not exists in index: {err:?}"))?;

        self.connection.query_row(
            "select id from buckets where name = ?1",
            rusqlite::params![name],
            |row| row.get(0),
        ).map_err(|err| error!("failed to find bucket by name in index: {err:?}"))
    }

    fn put_object(&self, bucket: &str, key: &[u8], key_hash: &str, size: u64) -> Result<(), ()> {
        let bucket_id = self.get_or_create_bucket(bucket)?;

        self.connection.execute(
            "insert into objects (bucket_id, key, key_hash, size) values (?1, ?2, ?3, ?4)
             on conflict (bucket_id, key) do update set key_hash = excluded.key_hash, size = excluded.size",
            rusqlite::params![bucket_id, key, key_hash, size as i64],
        ).map(|_| ()).map_err(|err| error!("failed to insert object into index: {err:?}"))
    }

    fn get_object(&self, bucket: &str, key: &[u8]) -> Result<Option<String>, ()> {
        let bucket_id: Option<i64> = self.connection.query_row(
            "select id from buckets where name = ?1",
            rusqlite::params![bucket],
            |row| row.get(0),
        ).optional().map_err(|err| error!("failed to check if bucket exists before getting object: {err:?}"))?;

        let bucket_id = match bucket_id {
            Some(v) => v,
            None => return Ok(None),
        };

        self.connection.query_row(
            "select key_hash from objects where bucket_id = ?1 and key = ?2",
            rusqlite::params![bucket_id, key],
            |row| row.get(0),
        ).optional().map_err(|err| error!("failed to get key hash from index: {err:?}"))
    }

    fn delete_object(&self, bucket: &str, key: &[u8]) -> Result<Option<String>, ()> {
        let bucket_id: Option<i64> = self.connection.query_row(
            "select id from buckets where name = ?1",
            rusqlite::params![bucket],
            |row| row.get(0),
        ).optional().map_err(|err| error!("failed to check if bucket exists before deleting object: {err:?}"))?;

        let bucket_id = match bucket_id {
            Some(v) => v,
            None => return Ok(None),
        };

        let key_hash: Option<String> = self.connection.query_row(
            "select key_hash from objects where bucket_id = ?1 and key = ?2",
            rusqlite::params![bucket_id, key],
            |row| row.get(0),
        ).optional().map_err(|err| error!("failed to check if object exists before deleting: {err:?}"))?;

        let key_hash = match key_hash {
            Some(v) => v,
            None => return Ok(None),
        };

        self.connection.execute(
            "delete from objects where bucket_id = ?1 and key = ?2",
            rusqlite::params![bucket_id, key],
        ).map_err(|err| error!("failed to delete object from index: {err:?}"))?;

        Ok(Some(key_hash))
    }
}
