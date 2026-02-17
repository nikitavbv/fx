use {
    std::iter::Iterator,
    thiserror::Error,
    fx_common::{SqlResultRow, SqlValue},
    fx_types::{capnp, abi_sql_capnp},
    crate::sys::DeserializeHostResource,
};

pub struct SqlResult {
    rows: Vec<SqlResultRow>,
}

#[derive(Debug, Error)]
pub enum SqlError {
    #[error("database is locked")]
    DatabaseBusy,
}

impl From<fx_common::SqlResult> for SqlResult {
    fn from(value: fx_common::SqlResult) -> Self {
        Self {
            rows: value.rows,
        }
    }
}

impl From<fx_types::capnp::struct_list::Reader<'_, fx_types::abi_capnp::sql_result_row::Owned>> for SqlResult {
    fn from(value: fx_types::capnp::struct_list::Reader<'_, fx_types::abi_capnp::sql_result_row::Owned>) -> Self {
        let rows = value.into_iter()
            .map(|v| SqlResultRow {
                columns: v.get_columns().unwrap()
                    .into_iter()
                    .map(|v| {
                        use fx_types::abi_capnp::sql_value::value::{Which as ProtocolValue};

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
