use {
    std::iter::Iterator,
    thiserror::Error,
    fx_types::{capnp, abi_sql_capnp},
    crate::sys::DeserializeHostResource,
};

#[derive(Clone, Debug)]
pub struct SqlQuery {
    pub stmt: String,
    pub params: Vec<SqlValue>,
}

impl SqlQuery {
    pub fn new(stmt: impl Into<String>) -> Self {
        Self {
            stmt: stmt.into(),
            params: Vec::new(),
        }
    }

    pub fn bind(mut self, param: impl IntoQueryParam) -> Self {
        self.params.push(param.into_query_param());
        self
    }
}

#[derive(Clone, Debug)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

pub struct SqlResult {
    rows: Vec<SqlResultRow>,
}

pub struct SqlResultRow {
    pub columns: Vec<SqlValue>,
}

pub(crate) struct SqlMigrations {
    pub database: String,
    pub migrations: Vec<String>,
}

#[derive(Debug, Error)]
pub enum SqlError {
    #[error("database is locked")]
    DatabaseBusy,
}

impl From<fx_types::capnp::struct_list::Reader<'_, fx_types::abi_sql_capnp::sql_result_row::Owned>> for SqlResult {
    fn from(value: fx_types::capnp::struct_list::Reader<'_, fx_types::abi_sql_capnp::sql_result_row::Owned>) -> Self {
        let rows = value.into_iter()
            .map(|v| SqlResultRow {
                columns: v.get_columns().unwrap()
                    .into_iter()
                    .map(|v| {
                        use fx_types::abi_sql_capnp::sql_value::value::{Which as ProtocolValue};

                        match v.get_value().which().unwrap() {
                            ProtocolValue::Null(_) => SqlValue::Null,
                            ProtocolValue::Integer(v) => SqlValue::Integer(v),
                            ProtocolValue::Real(v) => SqlValue::Real(v),
                            ProtocolValue::Text(v) => SqlValue::Text(v.unwrap().to_string().unwrap()),
                            ProtocolValue::Blob(v) => SqlValue::Blob(v.unwrap().to_vec()),
                        }
                    })
                    .collect()
            })
            .collect();

        Self {
            rows,
        }
    }
}

impl DeserializeHostResource for Result<SqlResult, SqlError> {
    fn deserialize(data: &mut &[u8]) -> Self {
        let resource_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let request = resource_reader.get_root::<fx_types::abi_sql_capnp::sql_exec_result::Reader>().unwrap();

        match request.get_result().which().unwrap() {
            abi_sql_capnp::sql_exec_result::result::Which::Rows(v) => Ok(SqlResult::from(v.unwrap())),
            abi_sql_capnp::sql_exec_result::result::Which::Error(_err) => Err(SqlError::DatabaseBusy),
        }
    }
}

impl SqlResult {
    pub fn rows(self) -> impl Iterator<Item = SqlResultRow> {
        self.rows.into_iter()
    }

    pub fn into_rows(self) -> Vec<SqlResultRow> {
        self.rows
    }
}

pub trait IntoQueryParam {
    fn into_query_param(self) -> SqlValue;
}

impl IntoQueryParam for i64 {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Integer(self)
    }
}

impl IntoQueryParam for &str {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Text(self.to_owned())
    }
}

impl IntoQueryParam for String {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Text(self)
    }
}

impl IntoQueryParam for Vec<u8> {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Blob(self)
    }
}

impl IntoQueryParam for bool {
    fn into_query_param(self) -> SqlValue {
        SqlValue::Integer(if self { 1 } else { 0 })
    }
}

impl<T: IntoQueryParam> IntoQueryParam for Option<T> {
    fn into_query_param(self) -> SqlValue {
        match self {
            Some(v) => v.into_query_param(),
            None => SqlValue::Null,
        }
    }
}

impl TryInto<String> for SqlValue {
    type Error = SqlMappingError;
    fn try_into(self) -> Result<String, Self::Error> {
        match self {
            Self::Text(text) => Ok(text),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryFrom<&SqlValue> for String {
    type Error = SqlMappingError;
    fn try_from(value: &SqlValue) -> Result<Self, Self::Error> {
        match value {
            SqlValue::Text(text) => Ok(text.clone()),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

impl TryFrom<&SqlValue> for u64 {
    type Error = SqlMappingError;
    fn try_from(value: &SqlValue) -> Result<Self, Self::Error> {
        match value {
            SqlValue::Integer(v) => Ok(*v as u64),
            _ => Err(SqlMappingError::WrongType),
        }
    }
}

#[derive(Error, Debug)]
pub enum SqlMappingError {
    #[error("this column cannot be converted to this type")]
    WrongType,
}
