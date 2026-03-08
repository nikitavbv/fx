use {
    std::path::PathBuf,
    rusqlite::OptionalExtension,
    tokio::{sync::oneshot, fs},
};

pub(crate) enum BlobMessage {
    Put {
        bucket: String,
        key: Vec<u8>,
        value: Vec<u8>,
        result: oneshot::Sender<()>,
    },
    Get {
        bucket: String,
        key: Vec<u8>,
        result: oneshot::Sender<Option<Vec<u8>>>,
    },
    Delete {
        bucket: String,
        key: Vec<u8>,
        result: oneshot::Sender<()>,
    },
}

pub(crate) fn run_blob_task(blob_path: PathBuf, blob_rx: flume::Receiver<BlobMessage>) {
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local_set = tokio::task::LocalSet::new();

    tokio_runtime.block_on(local_set.run_until(async {
        let data_path = blob_path.join("data");
        if !data_path.exists() {
            fs::create_dir_all(&data_path).await.unwrap();
        }

        let index_db = IndexDb::new(blob_path.join("index.sqlite"));

        while let Ok(msg) = blob_rx.recv_async().await {
            match msg {
                BlobMessage::Put { bucket, key, value, result } => {
                    let key_hash = hash_key_for_object(bucket.as_str(), &key);
                    index_db.put_object(&bucket, &key, &key_hash, value.len() as u64);

                    tokio::fs::write(key_path(&data_path, key_hash.as_str()).await.as_path(), value).await.unwrap();

                    result.send(()).unwrap();
                },
                BlobMessage::Get { bucket, key, result } => {
                    let key_hash = match index_db.get_object(&bucket, &key) {
                        Some(v) => v,
                        None => {
                            result.send(None).unwrap();
                            continue;
                        },
                    };

                    let value = tokio::fs::read(key_path(&data_path, key_hash.as_str()).await.as_path()).await.unwrap();

                    result.send(Some(value)).unwrap();
                },
                BlobMessage::Delete { bucket, key, result } => {
                    let key_hash = match index_db.delete_object(&bucket, &key) {
                        Some(v) => v,
                        None => {
                            result.send(()).unwrap();
                            continue;
                        }
                    };

                    tokio::fs::remove_file(key_path(&data_path, key_hash.as_str()).await.as_path()).await.unwrap();

                    result.send(()).unwrap();
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

async fn key_path(data_path: &PathBuf, key: &str) -> PathBuf {
    let dir_path = data_path.join(&key[0..2]).join(&key[2..4]);
    if !dir_path.exists() {
        fs::create_dir_all(&dir_path).await.unwrap();
    }

    dir_path.join(&key[4..])
}

struct IndexDb {
    connection: rusqlite::Connection,
}

impl IndexDb {
    fn new(path: PathBuf) -> Self {
        let mut connection = rusqlite::Connection::open(path).unwrap();
        connection.pragma_update(None, "journal_mode", "WAL").unwrap();
        connection.pragma_update(None, "synchronous", "NORMAL").unwrap();

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
        ]).to_latest(&mut connection).unwrap();

        Self {
            connection,
        }
    }

    fn get_or_create_bucket(&self, name: &str) -> i64 {
        self.connection.execute(
            "insert or ignore into buckets (name) values (?1)",
            rusqlite::params![name],
        ).unwrap();

        self.connection.query_row(
            "select id from buckets where name = ?1",
            rusqlite::params![name],
            |row| row.get(0),
        ).unwrap()
    }

    fn put_object(&self, bucket: &str, key: &[u8], key_hash: &str, size: u64) {
        let bucket_id = self.get_or_create_bucket(bucket);

        self.connection.execute(
            "insert into objects (bucket_id, key, key_hash, size) values (?1, ?2, ?3, ?4)
             on conflict (bucket_id, key) do update set key_hash = excluded.key_hash, size = excluded.size",
            rusqlite::params![bucket_id, key, key_hash, size as i64],
        ).unwrap();
    }

    fn get_object(&self, bucket: &str, key: &[u8]) -> Option<String> {
        let bucket_id: Option<i64> = self.connection.query_row(
            "select id from buckets where name = ?1",
            rusqlite::params![bucket],
            |row| row.get(0),
        ).optional().unwrap();

        let bucket_id = bucket_id?;

        self.connection.query_row(
            "select key_hash from objects where bucket_id = ?1 and key = ?2",
            rusqlite::params![bucket_id, key],
            |row| row.get(0),
        ).optional().unwrap()
    }

    fn delete_object(&self, bucket: &str, key: &[u8]) -> Option<String> {
        let bucket_id: Option<i64> = self.connection.query_row(
            "select id from buckets where name = ?1",
            rusqlite::params![bucket],
            |row| row.get(0),
        ).optional().unwrap();

        let bucket_id = bucket_id?;

        let key_hash: Option<String> = self.connection.query_row(
            "select key_hash from objects where bucket_id = ?1 and key = ?2",
            rusqlite::params![bucket_id, key],
            |row| row.get(0),
        ).optional().unwrap();

        let key_hash = key_hash?;

        self.connection.execute(
            "delete from objects where bucket_id = ?1 and key = ?2",
            rusqlite::params![bucket_id, key],
        ).unwrap();

        Some(key_hash)
    }
}
