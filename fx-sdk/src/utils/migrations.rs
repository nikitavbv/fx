use {
    thiserror::Error,
    fx_types::{capnp, abi_sql_capnp},
    crate::{SqlDatabase, sys::{fx_sql_migrate, FutureHostResource, OwnedResourceId, DeserializeHostResource}, sql::SqlMigrations},
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

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_sql_migrate(request.as_ptr() as u64, request.len() as u64) });
        let result: FutureHostResource<Result<(), SqlMigrationError>> = FutureHostResource::new(resource_id);

        result.await
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
    MigrationExecutionError { message: String },

    #[error("sql error: {message:?}")]
    SqlError { message: String },
}

impl DeserializeHostResource for Result<(), SqlMigrationError> {
    fn deserialize(data: &mut &[u8]) -> Self {
        let result_reader = capnp::serialize::read_message_from_flat_slice(data, capnp::message::ReaderOptions::default()).unwrap();
        let result = result_reader.get_root::<abi_sql_capnp::sql_migrate_result::Reader>().unwrap();

        match result.get_result().which().unwrap() {
            abi_sql_capnp::sql_migrate_result::result::Which::Ok(_) => Ok(()),
            abi_sql_capnp::sql_migrate_result::result::Which::Error(err) => Err(match err.unwrap().get_error().which().unwrap() {
                abi_sql_capnp::sql_migrate_error::error::Which::BindingNotFound(_) => SqlMigrationError::BindingNotFound,
                abi_sql_capnp::sql_migrate_error::error::Which::DatabaseBusy(_) => SqlMigrationError::DatabaseBusy,
                abi_sql_capnp::sql_migrate_error::error::Which::ExecutionError(message) => SqlMigrationError::MigrationExecutionError {
                    message: message.unwrap().to_string().unwrap(),
                },
                abi_sql_capnp::sql_migrate_error::error::Which::SqlError(message) => SqlMigrationError::SqlError {
                    message: message.unwrap().to_string().unwrap(),
                },
            }),
        }
    }
}
