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
