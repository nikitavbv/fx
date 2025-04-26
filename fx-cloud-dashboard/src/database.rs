use {
    fx::{SqlDatabase, SqlQuery},
    fx_utils::database::{sqlx_core::connection::ConnectOptions, FxDatabaseConnectOptions},
};

#[derive(Clone)]
pub struct Database {
    database: SqlDatabase,
}

impl Database {
    pub async fn new(database: SqlDatabase) -> Self {
        /*let pool = FxDatabaseConnectOptions::new("dashboard")
            .connect()
            .await
            .unwrap();*/

        Self { database }
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
        pub fn list_functions(&self) -> Vec<Function> {
            self.database.exec(SqlQuery::new("select function_id, total_invocations from functions"))
                .rows
                .into_iter()
                .map(|row| Function {
                    function_id: row.columns.get(0).unwrap().try_into().unwrap(),
                    total_invocations: row.columns.get(1).unwrap().try_into().unwrap(),
                })
                .collect()
        }
    }
}
