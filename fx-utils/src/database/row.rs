use {
    sqlx_core::row::Row,
    fx::SqlValue,
    super::{FxDatabase, FxDatabaseColumn, FxDatabaseValueRef, FxDatabaseValue},
};

#[derive(Debug)]
pub struct FxDatabaseRow {
    columns: Vec<FxDatabaseValue>,
}

impl FxDatabaseRow {
    pub fn new(columns: Vec<SqlValue>) -> Self {
        Self {
            columns: columns.into_iter()
                .map(|v| match v {
                    SqlValue::Integer(v) => FxDatabaseValue::Integer(v),
                    SqlValue::Text(v) => FxDatabaseValue::Text(v),
                    _ => unimplemented!(),
                })
                .collect(),
        }
    }
}

impl Row for FxDatabaseRow {
    type Database = FxDatabase;

    fn columns(&self) -> &[FxDatabaseColumn] {
        unimplemented!()
    }

    fn try_get_raw<I>(&self, index: I) -> Result<FxDatabaseValueRef<'_>, sqlx::Error> where I: sqlx::ColumnIndex<Self> {
        let index = index.index(&self).unwrap();
        Ok(FxDatabaseValueRef(self.columns.get(index).unwrap()))
    }
}
