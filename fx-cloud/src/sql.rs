use {
    std::sync::{Arc, Mutex},
    rusqlite::{Connection, params_from_iter, ToSql, types::ToSqlOutput},
};

pub(crate) struct Query {
    query: String,
    params: Vec<QueryParam>,
}

pub(crate) enum QueryParam {}

#[derive(Clone)]
pub struct Sql {
    connection: Arc<Mutex<Connection>>,
}

impl Sql {
    #[allow(dead_code)]
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        Self::from_connection(Connection::open(path).unwrap())
    }

    pub fn in_memory() -> Self {
        Self::from_connection(Connection::open_in_memory().unwrap())
    }

    fn from_connection(connection: Connection) -> Self {
        Self { connection: Arc::new(Mutex::new(connection)) }
    }

    pub fn exec(&self, query: Query) {
        let connection = self.connection.lock().unwrap();
        let mut stmt = connection.prepare(&query.query).unwrap();
        let mut rows = stmt.query(params_from_iter(query.params.into_iter())).unwrap();

        while let Some(row) = rows.next().unwrap() {
            // TOOD
        }
    }
}

impl ToSql for QueryParam {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match self {
            _ => unimplemented!()
        }
    }
}
