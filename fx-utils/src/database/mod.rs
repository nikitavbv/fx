use {
    std::{iter::Extend, fmt::Display, marker::PhantomData},
    sqlx::{Database, Connection},
    sqlx_core::{transaction::TransactionManager, row::Row, column::Column, type_info::TypeInfo, value::{Value, ValueRef}, arguments::Arguments, statement::Statement},
    self::connection::FxDatabaseConnection,
};

pub use {
    sqlx_core,
    self::connection::FxDatabaseConnectOptions,
};

mod connection;
mod error;

#[derive(Debug)]
pub struct FxDatabase {}

impl FxDatabase {
    pub fn new() -> Self {
        Self {}
    }
}

pub struct FxDatabaseTransactionManager;
pub struct FxDatabaseRow;
pub struct FxQueryResult;

#[derive(Debug)]
pub struct FxDatabaseColumn;

#[derive(PartialEq, Clone, Debug)]
pub struct FxDatabaseTypeInfo;

pub struct FxDatabaseValue;

pub struct FxDatabaseValueRef<'r> {
    _phantom: PhantomData<&'r str>,
}

#[derive(Debug)]
pub struct FxDatabaseArguments<'q> {
    phantom: PhantomData<&'q str>,
}

pub struct FxDatabaseArgumentValue<'q> {
    _phantom: PhantomData<&'q str>,
}

pub struct FxDatabaseStatement<'q> {
    _phantom: PhantomData<&'q str>,
}

impl Database for FxDatabase {
    type Connection = FxDatabaseConnection;
    type TransactionManager = FxDatabaseTransactionManager;
    type Row = FxDatabaseRow;
    type QueryResult = FxQueryResult;
    type Column = FxDatabaseColumn;
    type TypeInfo = FxDatabaseTypeInfo;
    type Value = FxDatabaseValue;
    type ValueRef<'r> = FxDatabaseValueRef<'r>;
    type Arguments<'q> = FxDatabaseArguments<'q>;
    type ArgumentBuffer<'q> = Vec<FxDatabaseArgumentValue<'q>>;
    type Statement<'q> = FxDatabaseStatement<'q>;
    const NAME: &'static str = "SQLite on Fx";
    const URL_SCHEMES: &'static [&'static str] = &["fx"];
}

impl TransactionManager for FxDatabaseTransactionManager {
    type Database = FxDatabase;

    fn begin<'conn>(
        conn: &'conn mut <Self::Database as Database>::Connection,
        statement: Option<std::borrow::Cow<'static, str>>,
    ) -> futures::future::BoxFuture<'conn, Result<(), sqlx::Error>> {
        unimplemented!()
    }

    fn commit(
        conn: &mut <Self::Database as Database>::Connection,
    ) -> futures::future::BoxFuture<'_, Result<(), sqlx::Error>> {
        unimplemented!()
    }

    fn rollback(
        conn: &mut <Self::Database as Database>::Connection,
    ) -> futures::future::BoxFuture<'_, Result<(), sqlx::Error>> {
        unimplemented!()
    }

    fn start_rollback(conn: &mut <Self::Database as Database>::Connection) {
        unimplemented!()
    }

    fn get_transaction_depth(conn: &<Self::Database as Database>::Connection) -> usize {
        unimplemented!()
    }
}

impl Row for FxDatabaseRow {
    type Database = FxDatabase;

    fn columns(&self) -> &[<Self::Database as Database>::Column] {
        unimplemented!()
    }

    fn try_get_raw<I>(&self, index: I) -> Result<<Self::Database as Database>::ValueRef<'_>, sqlx::Error> where I: sqlx::ColumnIndex<Self> {
        unimplemented!()
    }
}

impl Default for FxQueryResult {
    fn default() -> Self {
        Self
    }
}

impl Extend<FxQueryResult> for FxQueryResult {
    fn extend<T: IntoIterator<Item = FxQueryResult>>(&mut self, iter: T) {
        unimplemented!()
    }
}

impl Column for FxDatabaseColumn {
    type Database = FxDatabase;

    fn ordinal(&self) -> usize {
        unimplemented!()
    }

    fn name(&self) -> &str {
        unimplemented!()
    }

    fn type_info(&self) -> &<Self::Database as Database>::TypeInfo {
        unimplemented!()
    }
}

impl TypeInfo for FxDatabaseTypeInfo {
    fn is_null(&self) -> bool {
        unimplemented!()
    }

    fn name(&self) -> &str {
        unimplemented!()
    }
}

impl Display for FxDatabaseTypeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unimplemented!()
    }
}

impl Value for FxDatabaseValue {
    type Database = FxDatabase;

    fn as_ref(&self) -> <Self::Database as Database>::ValueRef<'_> {
        unimplemented!()
    }

    fn type_info(&self) -> std::borrow::Cow<'_, <Self::Database as Database>::TypeInfo> {
        unimplemented!()
    }

    fn is_null(&self) -> bool {
        unimplemented!()
    }
}

impl<'r> ValueRef<'r> for FxDatabaseValueRef<'r> {
    type Database = FxDatabase;

    fn to_owned(&self) -> <Self::Database as Database>::Value {
        unimplemented!()
    }

    fn type_info(&self) -> std::borrow::Cow<'_, <Self::Database as Database>::TypeInfo> {
        unimplemented!()
    }

    fn is_null(&self) -> bool {
        unimplemented!()
    }
}

impl<'q> Arguments<'q> for FxDatabaseArguments<'q> {
    type Database = FxDatabase;

    fn reserve(&mut self, additional: usize, size: usize) {
        unimplemented!()
    }

    fn add<T>(&mut self, value: T) -> Result<(), sqlx::error::BoxDynError> where T: 'q + sqlx::Encode<'q, Self::Database> + sqlx::Type<Self::Database> {
        unimplemented!()
    }

    fn len(&self) -> usize {
        unimplemented!()
    }
}

impl<'q> Default for FxDatabaseArguments<'q> {
    fn default() -> Self {
        Self {
            phantom: PhantomData,
        }
    }
}

impl<'q> Statement<'q> for FxDatabaseStatement<'q> {
    type Database = FxDatabase;
    fn to_owned(&self) -> <Self::Database as Database>::Statement<'static> {
        unimplemented!()
    }

    fn sql(&self) -> &str {
        unimplemented!()
    }

    fn parameters(&self) -> Option<sqlx::Either<&[<Self::Database as Database>::TypeInfo], usize>> {
        unimplemented!()
    }

    fn columns(&self) -> &[<Self::Database as Database>::Column] {
        unimplemented!()
    }

    fn query_as<O>(&self) -> sqlx::query::QueryAs<'_, Self::Database, O, <Self::Database as Database>::Arguments<'_>> where O: for<'r> sqlx::FromRow<'r, <Self::Database as Database>::Row> {
        unimplemented!()
    }

    fn query(&self) -> sqlx::query::Query<'_, Self::Database, <Self::Database as Database>::Arguments<'_>> {
        unimplemented!()
    }

    fn query_with<'s, A>(&'s self, arguments: A) -> sqlx::query::Query<'s, Self::Database, A> where A: sqlx::IntoArguments<'s, Self::Database> {
        unimplemented!()
    }

    fn query_as_with<'s, O, A>(&'s self, arguments: A) -> sqlx::query::QueryAs<'s, Self::Database, O, A> where O: for<'r> sqlx::FromRow<'r, <Self::Database as Database>::Row>, A: sqlx::IntoArguments<'s, Self::Database> {
        unimplemented!()
    }

    fn query_scalar<O>(&self,) -> sqlx::query::QueryScalar<'_, Self::Database, O, <Self::Database as Database>::Arguments<'_>> where (O,): for<'r> sqlx::FromRow<'r, <Self::Database as Database>::Row> {
        unimplemented!()
    }

    fn query_scalar_with<'s, O, A>(&'s self, arguments: A) -> sqlx::query::QueryScalar<'s, Self::Database, O, A> where (O,): for<'r> sqlx::FromRow<'r, <Self::Database as Database>::Row>, A: sqlx::IntoArguments<'s, Self::Database> {
        unimplemented!()
    }
}
