use {
    std::iter::Iterator,
    fx_core::SqlResultRow,
};

pub struct SqlResult {
    rows: Vec<SqlResultRow>,
}

impl From<fx_core::SqlResult> for SqlResult {
    fn from(value: fx_core::SqlResult) -> Self {
        Self {
            rows: value.rows,
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
