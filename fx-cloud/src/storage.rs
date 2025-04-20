use {
    std::sync::Arc,
    sqlite::{Connection, State},
};

pub trait KVStorage {
    fn set(&self, key: &[u8], value: &[u8]);
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;
}

#[derive(Clone)]
struct SqliteStorage {
    connection: Arc<Connection>,
}

impl SqliteStorage {
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        let connection = sqlite::open(path).unwrap();
        connection.execute("create table kv (key blob primary key, value blob primary key)").unwrap();
        Self { connection: Arc::new(connection) }
    }
}

impl KVStorage for SqliteStorage {
    fn set(&self, key: &[u8], value: &[u8]) {
        let mut stmt = self.connection.prepare("insert or replace into kv (key, value) values (:key, :value)").unwrap();
        stmt.bind(&[(":key", key), (":value", value)][..]).unwrap();
        while stmt.next().unwrap() != State::Done {}
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let mut stmt = self.connection.prepare("select value from kv where key = :key").unwrap();
        stmt.bind((":key", key)).unwrap();

        match stmt.next().unwrap() {
            State::Done => None,
            State::Row => Some(stmt.read("value").unwrap()),
        }
    }
}
