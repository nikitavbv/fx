use {
    thiserror::Error,
    rusqlite::{Connection, params_from_iter, types::{ValueRef, ToSqlOutput}, ToSql},
    crate::{
        function::abi::{capnp, abi_sql_capnp},
        resources::serialize::SerializeResource,
    },
};

#[derive(Debug)]
pub(crate) enum SqlQueryResult {
    Ok(Vec<SqlRow>),
    Error(SqlQueryExecutionError),
}

#[derive(Debug)]
pub struct SqlRow {
    pub columns: Vec<SqlValue>,
}

#[derive(Clone, Debug)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Debug)]
pub(crate) enum SqlMigrationResult {
    Ok(()),
    Error(SqlMigrationError),
}

#[derive(Debug, Error)]
enum SqlMigrationError {
    #[error("database is locked")]
    DatabaseBusy,
}

impl SerializeResource for SqlQueryResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let sql_exec_response = message.init_root::<abi_sql_capnp::sql_exec_result::Builder>();
        let sql_exec_response = sql_exec_response.init_result();

        match self {
            Self::Ok(rows) => {
                let mut response_rows = sql_exec_response.init_rows(rows.len() as u32);
                for (index, result_row) in rows.into_iter().enumerate() {
                    let mut response_row_columns = response_rows.reborrow().get(index as u32).init_columns(result_row.columns.len() as u32);
                    for (column_index, value) in result_row.columns.into_iter().enumerate() {
                        let mut response_value = response_row_columns.reborrow().get(column_index as u32).init_value();
                        match value {
                            SqlValue::Null => response_value.set_null(()),
                            SqlValue::Integer(v) => response_value.set_integer(v),
                            SqlValue::Real(v) => response_value.set_real(v),
                            SqlValue::Text(v) => response_value.set_text(v),
                            SqlValue::Blob(v) => response_value.set_blob(&v),
                        }
                    }
                }
            },
            Self::Error(err) => {
                let mut response_error = sql_exec_response.init_error();
                match err {
                    SqlQueryExecutionError::DatabaseBusy => response_error.set_database_busy(()),
                }
            }
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

impl SerializeResource for SqlMigrationResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let sql_migrate_result = message.init_root::<abi_sql_capnp::sql_migrate_result::Builder>();
        let mut sql_migrate_result = sql_migrate_result.init_result();

        match self {
            Self::Ok(_) => {
                sql_migrate_result.set_ok(());
            },
            Self::Error(err) => {
                let mut response_error = sql_migrate_result.init_error();
                match err {
                    SqlMigrationError::DatabaseBusy => response_error.set_database_busy(()),
                }
            }
        }

        capnp::serialize::write_message_to_words(&message)
    }
}


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

pub struct SqlDatabase {
    connection: Connection,
}

impl SqlDatabase {
    #[allow(dead_code)]
    pub fn new(path: impl AsRef<std::path::Path>) -> Result<Self, SqlError> {
        Ok(Self::from_connection(
            Connection::open(path)
                .map_err(|err| SqlError::ConnectionOpen {
                    reason: err.to_string(),
                })?
        ))
    }

    pub fn in_memory() -> Result<Self, SqlError> {
        Ok(Self::from_connection(
            Connection::open_in_memory()
                .map_err(|err| SqlError::ConnectionOpen { reason: err.to_string() })?
        ))
    }

    fn from_connection(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn exec(&self, query: Query) -> Result<QueryResult, SqlError> {
        let mut stmt = self.connection.prepare(&query.query)
            .map_err(|err| SqlError::QueryRun { reason: err.to_string() })?;
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
    pub fn batch(&mut self, statements: Vec<Query>) -> Result<(), SqlError> {
        let txn = self.connection.transaction()
            .map_err(|err| SqlError::TransactionStart { reason: err.to_string() })?;

        for query in statements {
            let mut stmt = txn.prepare(&query.query)
                .map_err(|err| SqlError::QueryRun { reason: err.to_string() })?;
            let _rows = stmt.query(params_from_iter(query.params.into_iter()))
                .map_err(|err| SqlError::QueryRun { reason: err.to_string() })?;
        }

        txn.commit().map_err(|err| SqlError::QueryRun { reason: err.to_string() })?;

        Ok(())
    }

    // run sql migrations
    pub fn migrate(&mut self, migrations: Vec<String>) -> Result<(), SqlError> {
        let mut rusqlite_migrations = Vec::new();
        for migration in &migrations {
            rusqlite_migrations.push(rusqlite_migration::M::up(migration));
        }

        let migrations = rusqlite_migration::Migrations::new(rusqlite_migrations);

        migrations.to_latest(&mut self.connection)
            .map_err(|err| SqlError::MigrationFailed { reason: err.to_string() })
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

    #[error("failed to run query: {reason}")]
    QueryRun { reason: String },

    #[error("failed to start transaction")]
    TransactionStart { reason: String },

    #[error("failed to acquire database connection")]
    ConnectionAcquire { reason: String },

    #[error("failed to open database connection")]
    ConnectionOpen { reason: String },

    #[error("sql migration failed: {reason}")]
    MigrationFailed { reason: String },
}

#[derive(Error, Debug)]
pub enum SqlMappingError {
    #[error("wrong type")]
    WrongType,
}

#[derive(Debug, Error)]
enum SqlQueryExecutionError {
    #[error("database is locked")]
    DatabaseBusy,
}
