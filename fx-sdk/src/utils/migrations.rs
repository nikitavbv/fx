use {
    fx_common::{SqlMigrations, FxSqlError},
    fx_types::{capnp, abi_sql_capnp},
    crate::{SqlDatabase, sys::{fx_sql_migrate, HostUnitFuture, OwnedResourceId}},
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

    pub async fn run(&self, database: &SqlDatabase) -> Result<(), FxSqlError> {
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

        HostUnitFuture::new(OwnedResourceId::from_ffi(unsafe { fx_sql_migrate(request.as_ptr() as u64, request.len() as u64) })).await;

        Ok(())
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
