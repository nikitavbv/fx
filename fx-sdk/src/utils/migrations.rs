use {
    fx_common::{SqlMigrations, FxSqlError},
    fx_types::{capnp, abi_capnp},
    crate::{SqlDatabase, sys::{self, invoke_fx_api}},
};

pub struct Migrations {
    migrations: Vec<Migration>,
}

impl Migrations {
    pub fn new() -> Self {
        Self {
            migrations: Vec::new(),
        }
    }

    pub fn with_migration(mut self, migration: Migration) -> Self {
        self.migrations.push(migration);
        self
    }

    pub fn run(&self, database: &SqlDatabase) -> Result<(), FxSqlError> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<abi_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut sql_migrate_request = op.init_sql_migrate();
        sql_migrate_request.set_database(&database.name);

        let mut migrations = sql_migrate_request.init_migrations(self.migrations.len() as u32);
        for (index, migration) in self.migrations.iter().enumerate() {
            migrations.set(index as u32, &migration.statement);
        }

        let response = invoke_fx_api(message);
        let response = response.get_root::<abi_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            abi_capnp::fx_api_call_result::op::Which::SqlMigrate(v) => {
                let migrate_response = v.unwrap();

                use abi_capnp::sql_migrate_response::response::Which;
                match migrate_response.get_response().which().unwrap() {
                    Which::Ok(_) => Ok(()),
                    Which::BindingNotFound(_) => Err(FxSqlError::BindingNotExists),
                    Which::SqlError(err) => Err(FxSqlError::QueryFailed { reason: err.unwrap().get_description().unwrap().to_string().unwrap() }),
                }
            },
            _other => panic!("unexpected response for migrate api"),
        }
    }
}

impl Default for Migrations {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Migration {
    statement: String,
}

impl Migration {
    pub fn new(statement: impl Into<String>) -> Self {
        Self {
            statement: statement.into(),
        }
    }
}
