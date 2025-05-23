use {
    fx::{SqlDatabase, SqlQuery},
    fx_utils::database::{sqlx_core::connection::ConnectOptions, FxDatabaseConnection, FxDatabaseConnectOptions, sqlx::{self, prelude::*}},
};

#[derive(Clone)]
pub struct Database {
    database: SqlDatabase,
    connection: FxDatabaseConnection, // TODO: migrate away from sqlx
}

impl Database {
    pub async fn new(database: SqlDatabase) -> Self {
        let connection = FxDatabaseConnectOptions::new(database.clone())
            .connect()
            .await
            .unwrap();

        Self { database, connection }
    }

    pub fn run_migrations(&self) {
        self.database.exec(SqlQuery::new("create table if not exists functions (function_id text primary key, total_invocations integer not null)".to_owned()));
    }
}

pub mod list_functions {
    use super::*;

    #[derive(Debug)]
    pub struct Function {
        pub function_id: String,
        pub total_invocations: u64,
    }

    impl Database {
        pub async fn list_functions(&self) -> Vec<Function> {
            sqlx::query("select function_id, total_invocations from functions")
                .fetch_all(&self.connection)
                .await
                .unwrap()
                .into_iter()
                .map(|row| Function {
                    function_id: row.get(0),
                    total_invocations: row.get(1),
                })
                .collect()
        }
    }
}
