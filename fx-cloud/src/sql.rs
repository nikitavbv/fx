use {
    std::sync::{Arc, Mutex},
    rusqlite::{Connection, params_from_iter, ToSql, types::{ToSqlOutput, ValueRef}},
};

pub(crate) struct Query {
    query: String,
    params: Vec<QueryParam>,
}

pub(crate) enum QueryParam {}

pub struct QueryResult {
    pub rows: Vec<Row>,
}

pub struct Row {
    pub columns: Vec<Value>,
}

pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Clone)]
pub struct SqlDatabase {
    connection: Arc<Mutex<Connection>>,
}

impl SqlDatabase {
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

    pub fn exec(&self, query: Query) -> QueryResult {
        let connection = self.connection.lock().unwrap();
        let mut stmt = connection.prepare(&query.query).unwrap();
        let result_columns = stmt.column_count();

        let mut rows = stmt.query(params_from_iter(query.params.into_iter())).unwrap();

        let mut result_rows = Vec::new();

        while let Some(row) = rows.next().unwrap() {
            let mut row_columns = Vec::new();
            for column in 0..result_columns {
                row_columns.push(match row.get_ref(column).unwrap() {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => Value::Integer(v),
                    ValueRef::Real(v) => Value::Real(v),
                    ValueRef::Text(v) => Value::Text(String::from_utf8(v.to_owned()).unwrap()),
                    ValueRef::Blob(v) => Value::Blob(v.to_owned()),
                });
            }
            result_rows.push(Row { columns: row_columns });
        }

        QueryResult { rows: result_rows }
    }
}

impl ToSql for QueryParam {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match self {
            _ => unimplemented!()
        }
    }
}
