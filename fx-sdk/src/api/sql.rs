use crate::sql::{SqlResult, SqlError};

pub(crate) struct SqlQueryResultFuture(u64);

impl SqlQueryResultFuture {
    pub fn new(resource_id: u64) -> Self {
        Self(resource_id)
    }
}

impl Future for SqlQueryResultFuture {
    type Output = Result<SqlResult, SqlError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        todo!()
    }
}
