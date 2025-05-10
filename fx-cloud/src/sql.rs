use {
    std::sync::{Arc, Mutex},
    rusqlite::{Connection, params_from_iter, ToSql, types::{ToSqlOutput, ValueRef}},
    thiserror::Error,
};

#[derive(Debug)]
pub struct Query {
    query: String,
    params: Vec<Value>,
}

impl Query {
    pub fn new(query: String) -> Self {
        Self { query, params: Vec::new() }
    }

    pub fn with_param(mut self, param: Value) -> Self {
        self.params.push(param);
        self
    }
}

#[derive(Debug)]
pub struct QueryResult {
    pub rows: Vec<Row>,
}

#[derive(Debug)]
pub struct Row {
    pub columns: Vec<Value>,
}

#[derive(Debug)]
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

    pub fn exec(&self, query: Query) -> Result<QueryResult, SqlError> {
        let connection = self.connection.lock().unwrap();
        let mut stmt = connection.prepare(&query.query).unwrap();
        let result_columns = stmt.column_count();

        let mut rows = stmt.query(params_from_iter(query.params.into_iter()))
            .map_err(|err| SqlError::QueryRun { reason: err.to_string() })?;

        let mut result_rows = Vec::new();

        while let Some(row) = rows.next().map_err(|err| SqlError::RowRead { reason: err.to_string() } )? {
            let mut row_columns = Vec::new();
            for column in 0..result_columns {
                let column = match row.get_ref(column) {
                    Ok(v) => v,
                    Err(err) => return Err(SqlError::ColumnGet { reason: err.to_string() }),
                };

                row_columns.push(match column {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => Value::Integer(v),
                    ValueRef::Real(v) => Value::Real(v),
                    ValueRef::Text(v) => Value::Text(
                        String::from_utf8(v.to_owned())
                            .map_err(|err| SqlError::FieldDecode { reason: err.to_string() })?,
                    ),
                    ValueRef::Blob(v) => Value::Blob(v.to_owned()),
                });
            }
            result_rows.push(Row { columns: row_columns });
        }

        Ok(QueryResult { rows: result_rows })
    }

    // run sql transaction
    pub fn batch(&self, statements: Vec<Query>) -> Result<(), SqlError> {
        let mut connection = self.connection.lock().unwrap();
        let txn = connection.transaction().unwrap();

        for query in statements {
            let mut stmt = txn.prepare(&query.query).unwrap();
            let _rows = stmt.query(params_from_iter(query.params.into_iter()))
                .map_err(|err| SqlError::QueryRun { reason: err.to_string() })?;
        }

        txn.commit().unwrap();

        Ok(())
    }
}

impl ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match self {
            Self::Null => None::<i64>.to_sql(),
            Self::Integer(v) => v.to_sql(),
            Self::Real(v) => v.to_sql(),
            Self::Text(v) => v.to_sql(),
            Self::Blob(v) => v.to_sql(),
        }
    }
}

impl TryInto<i64> for Value {
    type Error = SqlMappingError;
    fn try_into(self) -> Result<i64, Self::Error> {
        match self {
            Self::Integer(v) => Ok(v),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryInto<String> for Value {
    type Error = SqlMappingError;
    fn try_into(self) -> Result<String, Self::Error> {
        match self {
            Self::Text(v) => Ok(v),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryFrom<&Value> for i64 {
    type Error = SqlMappingError;
    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Integer(v) => Ok(*v),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryFrom<&Value> for String {
    type Error = SqlMappingError;
    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Text(v) => Ok(v.clone()),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

#[derive(Error, Debug)]
pub enum SqlError {
    #[error("failed to decode field")]
    FieldDecode { reason: String },

    #[error("failed to get column")]
    ColumnGet { reason: String },

    #[error("failed to read row")]
    RowRead { reason: String },

    #[error("failed to run query")]
    QueryRun { reason: String },
}

#[derive(Error, Debug)]
pub enum SqlMappingError {
    #[error("wrong type")]
    WrongType,
}
