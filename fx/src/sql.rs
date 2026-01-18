use {
    std::iter::Iterator,
    fx_common::{SqlResultRow, SqlValue},
};

pub struct SqlResult {
    rows: Vec<SqlResultRow>,
}

impl From<fx_common::SqlResult> for SqlResult {
    fn from(value: fx_common::SqlResult) -> Self {
        Self {
            rows: value.rows,
        }
    }
}

impl From<fx_types::capnp::struct_list::Reader<'_, fx_types::fx_capnp::sql_result_row::Owned>> for SqlResult {
    fn from(value: fx_types::capnp::struct_list::Reader<'_, fx_types::fx_capnp::sql_result_row::Owned>) -> Self {
        let rows = value.into_iter()
            .map(|v| SqlResultRow {
                columns: v.get_columns().unwrap()
                    .into_iter()
                    .map(|v| {
                        use fx_types::fx_capnp::sql_value::value::{Which as ProtocolValue};

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

impl SqlResult {
    pub fn rows(self) -> impl Iterator<Item = SqlResultRow> {
        self.rows.into_iter()
    }

    pub fn into_rows(self) -> Vec<SqlResultRow> {
        self.rows
    }
}
