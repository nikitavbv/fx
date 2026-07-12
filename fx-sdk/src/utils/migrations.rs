use {
    thiserror::Error,
    fx_types::{capnp, abi_sql_capnp},
    crate::{
        SqlDatabase,
        sys::fx_sql_migrate,
        api::sql::SqlMigrateResultFuture,
    },
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

    pub async fn run(&self, database: &SqlDatabase) -> Result<(), SqlMigrationError> {
        let request = {
            let mut message = capnp::message::Builder::new_default();
            let mut request = message.init_root::<abi_sql_capnp::sql_migrate_request::Builder>();
            request.set_binding(&database.name);

            let mut migrations = request.init_migrations(self.migrations.len() as u32);
            for (index, migration) in self.migrations.iter().enumerate() {
                migrations.set(index as u32, &migration.statement);
            }

            capnp::serialize::write_message_to_words(&message)
        };

        SqlMigrateResultFuture::new(unsafe { fx_sql_migrate(request.as_ptr() as u64, request.len() as u64) }).await
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

#[derive(Debug, Error)]
pub enum SqlMigrationError {
    #[error("database binding with this name is not found")]
    BindingNotFound,

    #[error("database is locked")]
    DatabaseBusy,

    #[error("failed to execute sql migration: {message:?}")]
    MigrationExecutionError { message: Option<String> },

    #[error("sql error: {message:?}")]
    SqlError { message: String },

    #[error("runtime is being shut down")]
    RuntimeShutdown,

    #[error("unknown error")]
    UnknownError,

    #[error("internal sdk error")]
    InternalSdkError,

    #[error("internal runtime error")]
    RuntimeError,
}
