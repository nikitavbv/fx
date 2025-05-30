use {
    fx_core::SqlMigrations,
    crate::{SqlDatabase, sys},
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

    pub fn run(&self, database: &SqlDatabase) {
        let migrations = SqlMigrations {
            database: database.name.clone(),
            migrations: self.migrations.iter()
                .map(|migration| migration.statement.clone())
                .collect(),
        };
        let migrations = rmp_serde::to_vec(&migrations).unwrap();
        unsafe { sys::sql_migrate(migrations.as_ptr() as i64, migrations.len() as i64); }
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
