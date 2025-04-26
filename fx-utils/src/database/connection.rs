use {
    std::str::FromStr,
    sqlx_core::connection::{Connection, ConnectOptions},
    futures::future::BoxFuture,
    log::LevelFilter,
    super::FxDatabase,
};

pub struct FxDatabaseConnection;

impl Connection for FxDatabaseConnection {
    type Database = FxDatabase;

    type Options = FxDatabaseConnectOptions;

    fn close(self) -> BoxFuture<'static, Result<(), sqlx::Error>> {
        unimplemented!()
    }

    fn close_hard(self) -> BoxFuture<'static, Result<(), sqlx::Error>> {
        unimplemented!()
    }

    fn ping(&mut self) -> BoxFuture<'_, Result<(), sqlx::Error>> {
        unimplemented!()
    }

    fn begin(&mut self) -> BoxFuture<'_, Result<sqlx::Transaction<'_, Self::Database>, sqlx::Error>> where Self: Sized {
        unimplemented!()
    }

    fn shrink_buffers(&mut self) {
        unimplemented!()
    }

    fn flush(&mut self) -> BoxFuture<'_, Result<(), sqlx::Error>> {
        unimplemented!()
    }

    fn should_flush(&self) -> bool {
        unimplemented!()
    }
}

#[derive(Clone, Debug)]
pub struct FxDatabaseConnectOptions;

impl ConnectOptions for FxDatabaseConnectOptions {
    type Connection = FxDatabaseConnection;

    fn from_url(url: &sqlx_core::Url) -> Result<Self, sqlx::Error> {
        unimplemented!()
    }

    fn connect(&self) -> BoxFuture<'_, Result<Self::Connection, sqlx::Error>> where Self::Connection: Sized {
        unimplemented!()
    }

    fn log_statements(self, level: LevelFilter) -> Self {
        unimplemented!()
    }

    fn log_slow_statements(self, level: LevelFilter, duration: std::time::Duration) -> Self {
        unimplemented!()
    }
}

impl FromStr for FxDatabaseConnectOptions {
    type Err = sqlx::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        unimplemented!()
    }
}
