use {
    std::{collections::HashMap, path::PathBuf},
    tracing::debug,
    tokio::sync::oneshot,
    thiserror::Error,
    crate::{
        definitions::bindings::{SqlBindingConfig, SqlBindingConfigLocation},
        effects::sql::{SqlValue, SqlQueryExecutionError, SqlRow},
    },
};

#[derive(Debug)]
pub(crate) enum SqlMessage {
    Migrate(SqlMigrateMessage),
    Exec(SqlExecMessage),
    Batch(SqlBatchMessage),
}

#[derive(Debug)]
pub(crate) struct SqlExecMessage {
    pub(crate) binding: SqlBindingConfig,
    pub(crate) statement: String,
    pub(crate) params: Vec<SqlValue>,
    pub(crate) response: oneshot::Sender<Result<Vec<SqlRow>, SqlQueryExecutionError>>,
}

#[derive(Debug)]
pub(crate) struct SqlMigrateMessage {
    pub(crate) binding: SqlBindingConfig,
    pub(crate) migrations: Vec<String>,
    pub(crate) response: oneshot::Sender<Result<(), SqlTaskMigrationError>>,
}

#[derive(Debug)]
pub(crate) struct SqlBatchMessage {
    pub(crate) binding: SqlBindingConfig,
    pub(crate) queries: Vec<(String, Vec<SqlValue>)>,
    pub(crate) response: oneshot::Sender<Result<(), SqlTaskBatchError>>,
}

#[derive(Debug, Error)]
pub(crate) enum SqlConnectionInitError {
    #[error("database is locked")]
    DatabaseBusy,
}

#[derive(Debug, Error)]
pub(crate) enum SqlTaskMigrationError {
    #[error("binding with this name is not found")]
    BindingNotFound,
    #[error("database is locked")]
    DatabaseBusy,
    #[error("migration execution error: {message:?}")]
    MigrationExecutionError {
        message: String,
    },
    /// SqlError is very similar to MigrationExecutionError, but with more information available
    #[error("sql error: {message:?}")]
    SqlError {
        message: String,
    },
}

#[derive(Debug, Error)]
pub(crate) enum SqlTaskBatchError {
    #[error("binding with this name is not found")]
    BindingNotFound,
    #[error("database is locked")]
    DatabaseBusy,
    #[error("statement failed: {reason:?}")]
    StatementFailed { reason: String },
}

pub(crate) fn run_sql_task(worker_index: u64, total_workers: u64, databases_path: PathBuf, sql_rx: flume::Receiver<SqlMessage>) {
    use rusqlite::types::ValueRef;

    let mut connections = HashMap::<String, rusqlite::Connection>::new();

    if !databases_path.exists() {
        std::fs::create_dir_all(&databases_path).unwrap();
    }

    while let Ok(msg) = sql_rx.recv() {
        let binding = match &msg {
            SqlMessage::Exec(v) => &v.binding,
            SqlMessage::Batch(v) => &v.binding,
            SqlMessage::Migrate(v) => &v.binding,
        };
        let connection_id = binding.connection_id.clone();
        let busy_timeout = binding.busy_timeout.clone();

        let connection = match connections.entry(binding.connection_id.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                debug!(database=connection_id, "creating new connection");
                let connection = match &binding.location {
                    SqlBindingConfigLocation::InMemory(v) => rusqlite::Connection::open_with_flags(
                        format!("file:{v}"),
                        rusqlite::OpenFlags::default()
                            .union(rusqlite::OpenFlags::SQLITE_OPEN_MEMORY)
                            .union(rusqlite::OpenFlags::SQLITE_OPEN_SHARED_CACHE)
                    ).unwrap(),
                    SqlBindingConfigLocation::DatabaseId(database_id) => {
                        let mut database_path = databases_path.join(database_id);
                        database_path.add_extension("sqlite");
                        rusqlite::Connection::open(database_path).unwrap()
                    },
                };
                debug!(database=connection_id, "created connection");
                if let Some(busy_timeout) = binding.busy_timeout {
                    connection.busy_timeout(busy_timeout).unwrap();
                }
                let result = if let Err(err) = connection.pragma_update(None, "journal_mode", "WAL") {
                    if is_database_busy_error(&err) {
                        Err(SqlConnectionInitError::DatabaseBusy)
                    } else {
                        todo!("unhandled sqlite error: {err:?}")
                    }
                } else {
                    if let Err(err) = connection.pragma_update(None, "synchronous", "NORMAL") {
                        if is_database_busy_error(&err) {
                            Err(SqlConnectionInitError::DatabaseBusy)
                        } else {
                            todo!("unhandled sqlite error: {err:?}")
                        }
                    } else {
                        Ok(entry.insert(connection))
                    }
                };
                debug!(database=connection_id, "connection setup finished");
                result
            }
        };

        match msg {
            SqlMessage::Exec(msg) => {
                debug!(database=connection_id, "running sql exec");
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlConnectionInitError::DatabaseBusy => {
                            msg.response.send(Err(SqlQueryExecutionError::DatabaseBusy)).unwrap();
                            continue;
                        }
                    }
                };

                let mut stmt = match connection.prepare(&msg.statement) {
                    Ok(v) => v,
                    Err(err) => {
                        if is_database_busy_error(&err) {
                            msg.response.send(Err(SqlQueryExecutionError::DatabaseBusy)).unwrap();
                            continue;
                        } else if let Some(statement_error) = is_statement_error(&err) {
                            msg.response.send(Err(SqlQueryExecutionError::StatementError(statement_error.clone()))).unwrap();
                            continue;
                        } else {
                            todo!("unhandled sqlite error: {err:?}");
                        }
                    }
                };
                let result_columns = stmt.column_count();

                let mut rows = stmt.query(rusqlite::params_from_iter(msg.params.into_iter())).unwrap();

                let mut result_rows = Vec::new();
                let response_message = loop {
                    let row = rows.next();
                    let row = match row {
                        Ok(v) => v,
                        Err(err) => {
                            if is_database_busy_error(&err) {
                                break Err(SqlQueryExecutionError::DatabaseBusy);
                            } else {
                                panic!("unexpected sqlite error: {err:?}")
                            }
                        }
                    };
                    let row = match row {
                        Some(v) => v,
                        None => break Ok(result_rows),
                    };

                    let mut row_columns = Vec::new();
                    for column in 0..result_columns {
                        let column = row.get_ref(column).unwrap();

                        row_columns.push(match column {
                            ValueRef::Null => SqlValue::Null,
                            ValueRef::Integer(v) => SqlValue::Integer(v),
                            ValueRef::Real(v) => SqlValue::Real(v),
                            ValueRef::Text(v) => SqlValue::Text(
                                String::from_utf8(v.to_owned()).unwrap()
                            ),
                            ValueRef::Blob(v) => SqlValue::Blob(v.to_owned()),
                        });
                    }
                    result_rows.push(SqlRow { columns: row_columns });
                };

                debug!(database=connection_id, "running sql exec - done");
                msg.response.send(response_message).unwrap();
            },
            SqlMessage::Batch(msg) => {
                debug!(database=connection_id, "running sql batch");
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlConnectionInitError::DatabaseBusy => {
                            msg.response.send(Err(SqlTaskBatchError::DatabaseBusy)).unwrap();
                            continue;
                        }
                    },
                };

                let txn = match connection.transaction() {
                    Ok(v) => v,
                    Err(err) => {
                        if is_database_busy_error(&err) {
                            msg.response.send(Err(SqlTaskBatchError::DatabaseBusy)).unwrap();
                            continue;
                        } else {
                            panic!("unexpected sqlite error: {err:?}");
                        }
                    }
                };


                let mut execution_result = Ok(());
                for (statement, params) in msg.queries {
                    match txn.execute(&statement, rusqlite::params_from_iter(params.into_iter())) {
                        Ok(_) => {},
                        Err(rusqlite::Error::SqliteFailure(_, Some(reason))) => {
                            execution_result = Err(SqlTaskBatchError::StatementFailed { reason });
                        },
                        Err(rusqlite::Error::SqlInputError { error: _, msg: reason, sql: _, offset: _ }) => {
                            execution_result = Err(SqlTaskBatchError::StatementFailed { reason });
                        },
                        Err(err) => panic!("unexpected sqlite error: {err:?}"),
                    }
                };

                let response = execution_result
                    .and_then(|_| txn.commit().map_err(|err| {
                        let error_code = err.sqlite_error().unwrap().code;
                        if error_code == rusqlite::ErrorCode::DatabaseBusy {
                            SqlTaskBatchError::DatabaseBusy
                        } else {
                            panic!("unexpected sqlite error: {err:?}");
                        }
                    }));

                debug!(database=connection_id, "running sql batch - done.");
                msg.response.send(response).unwrap();
            },
            SqlMessage::Migrate(msg) => {
                debug!(database=connection_id, "running migration, busy timeout: {busy_timeout:?}");
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlConnectionInitError::DatabaseBusy => {
                            debug!("database busy when getting connection to run migration");
                            msg.response.send(Err(SqlTaskMigrationError::DatabaseBusy)).unwrap();
                            continue;
                        }
                    }
                };

                let mut rusqlite_migrations = Vec::new();
                for migration in &msg.migrations {
                    rusqlite_migrations.push(rusqlite_migration::M::up(migration));
                }

                let migrations = rusqlite_migration::Migrations::new(rusqlite_migrations);

                let response = match migrations.to_latest(connection) {
                    Ok(_) => Ok(()),
                    Err(err) => match err {
                        rusqlite_migration::Error::RusqliteError { query: _, err } => match err {
                            rusqlite::Error::SqliteFailure(error, message) => match error.code {
                                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                                    debug!("database busy when running migration");
                                    Err(SqlTaskMigrationError::DatabaseBusy)
                                },
                                rusqlite::ErrorCode::Unknown => Err(SqlTaskMigrationError::MigrationExecutionError {
                                    message: message.unwrap(),
                                }),
                                other => panic!("unexpected sqlite error: {other:?}"),
                            },
                            rusqlite::Error::SqlInputError { error: _, msg, sql: _, offset: _ } => Err(SqlTaskMigrationError::SqlError {
                                message: msg,
                            }),
                            other => panic!("unexpected sqlite error: {other:?}"),
                        },
                        other => panic!("unexpected sqlite error: {other:?}"),
                    }
                };

                debug!(database=connection_id, "running migration - done.");
                msg.response.send(response).unwrap();
            },
        }
    }
}

fn is_database_busy_error(err: &rusqlite::Error) -> bool {
    let error_code = err.sqlite_error().unwrap().code;
    error_code == rusqlite::ErrorCode::DatabaseBusy || error_code == rusqlite::ErrorCode::DatabaseLocked
}

// triggered when there is a problem with sql statement, for example more values than columns when inserting
fn is_statement_error(err: &rusqlite::Error) -> Option<&String> {
    match err {
        rusqlite::Error::SqliteFailure(_, err) => err.as_ref(),
        _ => None,
    }
}
