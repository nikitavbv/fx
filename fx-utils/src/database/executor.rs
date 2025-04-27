use {
    std::pin::Pin,
    sqlx::Executor,
    sqlx_core::{describe::Describe, column::ColumnIndex},
    futures::{stream::BoxStream, future::BoxFuture},
    super::{connection::FxDatabaseConnection, FxDatabase, FxDatabaseRow},
};

impl<'a> Executor<'a> for &'a FxDatabaseConnection {
    type Database = FxDatabase;

    fn fetch<'e, 'q: 'e, E>(self, query: E,) -> BoxStream<'e, Result<<Self::Database as sqlx::Database>::Row, sqlx::Error>> where 'a: 'e, E: 'q + sqlx::Execute<'q, Self::Database>, {
        unimplemented!()
    }

    fn execute<'e, 'q: 'e, E>(self, query: E,) -> BoxFuture<'e, Result<<Self::Database as sqlx::Database>::QueryResult, sqlx::Error>> where 'a: 'e, E: 'q + sqlx::Execute<'q, Self::Database>, {
        unimplemented!()
    }

    fn prepare<'e, 'q: 'e>(self, query: &'q str,) -> BoxFuture<'e, Result<<Self::Database as sqlx::Database>::Statement<'q>, sqlx::Error>> where 'a: 'e, {
        unimplemented!()
    }

    fn fetch_all<'e, 'q: 'e, E>(self, mut query: E,) -> BoxFuture<'e, Result<Vec<<Self::Database as sqlx::Database>::Row>, sqlx::Error>> where 'a: 'e, E: 'q + sqlx::Execute<'q, Self::Database>, {
        let sql = query.sql();
        let fx_query = fx::SqlQuery::new(sql);

        let arguments = query.take_arguments().unwrap();
        // TODO: handle arguments

        let result = self.database.exec(fx_query);
        let rows = result.rows.into_iter()
            .map(|row| FxDatabaseRow::new(row.columns))
            .collect();

        Box::pin(async move { Ok(rows) })
    }

    fn fetch_one<'e, 'q: 'e, E>(self, query: E,) -> BoxFuture<'e, Result<<Self::Database as sqlx::Database>::Row, sqlx::Error>> where 'a: 'e, E: 'q + sqlx::Execute<'q, Self::Database>, {
        unimplemented!()
    }

    fn fetch_many<'e, 'q: 'e, E>(self, query: E,) -> BoxStream<'e, Result<sqlx::Either<<Self::Database as sqlx::Database>::QueryResult, <Self::Database as sqlx::Database>::Row>, sqlx::Error,>,> where 'a: 'e, E: 'q + sqlx::Execute<'q, Self::Database> {
        unimplemented!()
    }

    fn execute_many<'e, 'q: 'e, E>(self, query: E,) -> BoxStream<'e, Result<<Self::Database as sqlx::Database>::QueryResult, sqlx::Error>> where 'a: 'e, E: 'q + sqlx::Execute<'q, Self::Database>, {
        unimplemented!()
    }

    fn fetch_optional<'e, 'q: 'e, E>(self, query: E,) -> BoxFuture<'e, Result<Option<<Self::Database as sqlx::Database>::Row>, sqlx::Error>> where 'a: 'e, E: 'q + sqlx::Execute<'q, Self::Database> {
        unimplemented!()
    }

    fn prepare_with<'e, 'q: 'e>(self, sql: &'q str, parameters: &'e [<Self::Database as sqlx::Database>::TypeInfo],) -> BoxFuture<'e, Result<<Self::Database as sqlx::Database>::Statement<'q>, sqlx::Error>> where 'a: 'e {
        unimplemented!()
    }

    fn describe<'e, 'q: 'e>(self, _: &'q str) -> Pin<Box<(dyn futures::Future<Output = Result<Describe<<Self as Executor<'a>>::Database>, sqlx::Error>> + std::marker::Send + 'e)>> { todo!() }
}

impl ColumnIndex<FxDatabaseRow> for u64 {
    fn index(&self, _container: &FxDatabaseRow) -> Result<usize, sqlx::Error> {
        Ok(*self as usize)
    }
}
