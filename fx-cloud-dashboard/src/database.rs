use {
    fx::{SqlDatabase, SqlQuery, SqlValue},
};

#[derive(Clone)]
pub struct Database {
    database: SqlDatabase,
}

impl Database {
    pub async fn new(database: SqlDatabase) -> Self {
        Self { database }
    }

    pub fn run_migrations(&self) {
        self.database.exec(SqlQuery::new("create table if not exists functions (function_id text primary key, total_invocations integer not null)".to_owned())).unwrap();
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
            self.database.exec(SqlQuery::new("select function_id, total_invocations from functions")).unwrap()
                .rows
                .into_iter()
                .map(|row| Function {
                    function_id: match row.columns.get(0).unwrap() {
                        SqlValue::Text(v) => v.to_owned(),
                        _ => panic!("unexpected type"),
                    },
                    total_invocations: match row.columns.get(1).unwrap() {
                        SqlValue::Integer(v) => *v as u64,
                        _ => panic!("unexpected type"),
                    },
                })
                .collect()
        }
    }
}
