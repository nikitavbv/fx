use {
    thiserror::Error,
    rusqlite::{Connection, params_from_iter, types::{ValueRef, ToSqlOutput}, ToSql},
    crate::tasks::sql::{SqlTaskBatchError, SqlTaskMigrationError},
};

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

#[derive(Debug, Error)]
pub(crate) enum SqlMigrationError {
    #[error("binding with this name is not found")]
    BindingNotFound,
    #[error("database is locked")]
    DatabaseBusy,
    #[error("migration execution error: {message:?}")]
    MigrationExecutionError {
        message: String,
    },
    /// SqlError is very similar to MigrationExecutionError, but with more information available
    #[error("sql error: {message:?}")]
    SqlError {
        message: String,
    },
    #[error("runtime is being shut down")]
    RuntimeShutdown,
}

impl From<SqlTaskMigrationError> for SqlMigrationError {
    fn from(err: SqlTaskMigrationError) -> Self {
        match err {
            SqlTaskMigrationError::DatabaseBusy => Self::DatabaseBusy,
            SqlTaskMigrationError::MigrationExecutionError { message } => Self::MigrationExecutionError { message },
            SqlTaskMigrationError::SqlError { message } => Self::SqlError { message },
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum SqlBatchError {
    #[error("binding with this name is not found")]
    BindingNotFound,
    #[error("database is locked")]
    DatabaseBusy,
    #[error("statement failed: {reason:?}")]
    StatementFailed { reason: String },
    #[error("runtime is being shut down")]
    RuntimeShutdown,
}

impl From<SqlTaskBatchError> for SqlBatchError {
    fn from(err: SqlTaskBatchError) -> Self {
        match err {
            SqlTaskBatchError::DatabaseBusy => Self::DatabaseBusy,
            SqlTaskBatchError::StatementFailed { reason } => Self::StatementFailed { reason },
        }
    }
}

#[derive(Debug)]
pub struct Query {
    query: String,
    params: Vec<SqlValue>,
}

impl Query {
    pub fn new(query: String) -> Self {
        Self { query, params: Vec::new() }
    }

    pub fn with_param(mut self, param: SqlValue) -> Self {
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
    pub columns: Vec<SqlValue>,
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
                    ValueRef::Null => SqlValue::Null,
                    ValueRef::Integer(v) => SqlValue::Integer(v),
                    ValueRef::Real(v) => SqlValue::Real(v),
                    ValueRef::Text(v) => SqlValue::Text(
                        String::from_utf8(v.to_owned())
                            .map_err(|err| SqlError::FieldDecode { reason: err.to_string() })?,
                    ),
                    ValueRef::Blob(v) => SqlValue::Blob(v.to_owned()),
                });
            }
            result_rows.push(Row { columns: row_columns });
        }

        Ok(QueryResult { rows: result_rows })
    }
}

impl ToSql for SqlValue {
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

impl TryInto<i64> for SqlValue {
    type Error = SqlMappingError;
    fn try_into(self) -> Result<i64, Self::Error> {
        match self {
            Self::Integer(v) => Ok(v),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryInto<String> for SqlValue {
    type Error = SqlMappingError;
    fn try_into(self) -> Result<String, Self::Error> {
        match self {
            Self::Text(v) => Ok(v),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryFrom<&SqlValue> for i64 {
    type Error = SqlMappingError;
    fn try_from(value: &SqlValue) -> Result<Self, Self::Error> {
        match value {
            SqlValue::Integer(v) => Ok(*v),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryFrom<&SqlValue> for String {
    type Error = SqlMappingError;
    fn try_from(value: &SqlValue) -> Result<Self, Self::Error> {
        match value {
            SqlValue::Text(v) => Ok(v.clone()),
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

    #[error("failed to open database connection")]
    ConnectionOpen { reason: String },
}

#[derive(Error, Debug)]
pub enum SqlMappingError {
    #[error("wrong type")]
    WrongType,
}

/// Error produced by sql_query api call
#[derive(Debug, Error)]
pub(crate) enum SqlQueryError {
    #[error("binding with this name is not found")]
    BindingNotFound,
    #[error("database is locked")]
    DatabaseBusy,
    #[error("runtime is being shutdown")]
    RuntimeShutdown,
    #[error("sql statement error: {0:?}")]
    StatementError(String),
}

/// SqlQueryExecutionError is a subset of SqlQueryError
impl From<SqlQueryExecutionError> for SqlQueryError {
    fn from(value: SqlQueryExecutionError) -> Self {
        match value {
            SqlQueryExecutionError::DatabaseBusy => Self::DatabaseBusy,
            SqlQueryExecutionError::StatementError(reason) => Self::StatementError(reason),
        }
    }
}

/// Error that occured while executing sql query
#[derive(Debug, Error)]
pub(crate) enum SqlQueryExecutionError {
    #[error("database is locked")]
    DatabaseBusy,
    #[error("sql statement error: {0:?}")]
    StatementError(String),
}
