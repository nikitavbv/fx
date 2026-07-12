use {
    std::{collections::HashMap, path::PathBuf},
    tracing::{debug, error},
    tokio::sync::oneshot,
    thiserror::Error,
    crate::{
        definitions::bindings::{SqlBindingConfig, SqlBindingConfigLocation},
        effects::sql::{SqlValue, SqlQueryExecutionError, SqlRow},
    },
};

pub(crate) use self::controller::SqlController;

mod controller;

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
    #[error("failed to init sql connection for unknown reasons")]
    UnknownError,
}

#[derive(Debug, Error)]
pub(crate) enum SqlTaskMigrationError {
    #[error("database is locked")]
    DatabaseBusy,
    #[error("migration execution error: {message:?}")]
    MigrationExecutionError {
        message: Option<String>,
    },
    /// SqlError is very similar to MigrationExecutionError, but with more information available
    #[error("sql error: {message:?}")]
    SqlError {
        message: String,
    },
    #[error("unknown error in sql task")]
    UnknownError,
}

#[derive(Debug, Error)]
pub(crate) enum SqlTaskBatchError {
    #[error("database is locked")]
    DatabaseBusy,
    #[error("statement failed: {reason:?}")]
    StatementFailed { reason: String },
    #[error("unknown error in sql task")]
    UnknownError,
}

pub(crate) fn run_sql_task(databases_path: PathBuf, sql_rx: flume::Receiver<SqlMessage>, sql_thread_rx: flume::Receiver<SqlMessage>) {
    use rusqlite::types::ValueRef;

    let mut connections = HashMap::<String, rusqlite::Connection>::new();

    if !databases_path.exists() {
        std::fs::create_dir_all(&databases_path).unwrap();
    }


    while let Ok(msg) = flume::Selector::new()
        .recv(&sql_rx, |v| v)
        .recv(&sql_thread_rx, |v| v).wait() {
        let binding = match &msg {
            SqlMessage::Exec(v) => &v.binding,
            SqlMessage::Batch(v) => &v.binding,
            SqlMessage::Migrate(v) => &v.binding,
        };
        let connection_id = binding.connection_id.clone();
        let busy_timeout = binding.busy_timeout;

        let connection = match connections.entry(binding.connection_id.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                debug!(database=connection_id, "creating new connection");
                let connection = match &binding.location {
                    SqlBindingConfigLocation::InMemory(v) => rusqlite::Connection::open_with_flags(
                        format!("file:/{v}?vfs=memdb"), // slash in front is needed to make this database shared between threads!
                        rusqlite::OpenFlags::default()
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
                            error!("unknown sqlite error when updating pragma for \"synchronous\": {err:?}");
                            Err(SqlConnectionInitError::UnknownError)
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
                            // error can be ignored here because it means that request was cancelled
                            let _ = msg.response.send(Err(SqlQueryExecutionError::DatabaseBusy));
                            continue;
                        },
                        SqlConnectionInitError::UnknownError => {
                            // error can be ignored here because it means that request was cancelled
                            let _ = msg.response.send(Err(SqlQueryExecutionError::UnknownError));
                            continue;
                        },
                    }
                };

                let mut stmt = match connection.prepare(&msg.statement) {
                    Ok(v) => v,
                    Err(err) => {
                        if is_database_busy_error(&err) {
                            // error can be ignored here because it means that request was cancelled
                            let _ = msg.response.send(Err(SqlQueryExecutionError::DatabaseBusy));
                        } else if let Some(statement_error) = is_statement_error(&err) {
                            // error can be ignored here because it means that request was cancelled
                            let _ = msg.response.send(Err(SqlQueryExecutionError::StatementError(statement_error.clone())));
                        } else {
                            error!("unknown sqlite error when preparing connection: {err:?}");
                            // error can be ignored here because it means that request was cancelled
                            let _ = msg.response.send(Err(SqlQueryExecutionError::UnknownError));
                        }
                        continue;
                    }
                };
                let result_columns = stmt.column_count();

                let mut rows = match stmt.query(rusqlite::params_from_iter(msg.params)) {
                    Ok(v) => v,
                    Err(err) => {
                        error!("failed to create sql query: {err:?}");
                        // error can be ignored here because it means that request has been cancelled
                        let _ = msg.response.send(Err(SqlQueryExecutionError::UnknownError));
                        continue;
                    }
                };

                let mut result_rows = Vec::new();
                let response_message = 'rows_loop: loop {
                    let row = rows.next();
                    let row = match row {
                        Ok(v) => v,
                        Err(err) => {
                            break Err(if is_database_busy_error(&err) {
                                SqlQueryExecutionError::DatabaseBusy
                            } else {
                                error!("unexpected error when reading rows from query response: {err:?}");
                                SqlQueryExecutionError::UnknownError
                            });
                        }
                    };
                    let row = match row {
                        Some(v) => v,
                        None => break Ok(result_rows),
                    };

                    let mut row_columns = Vec::new();
                    for column in 0..result_columns {
                        let column = match row.get_ref(column) {
                            Ok(v) => v,
                            Err(err) => {
                                error!("unexpected error when getting column value: {err:?}");
                                break 'rows_loop Err(SqlQueryExecutionError::UnknownError);
                            },
                        };

                        row_columns.push(match column {
                            ValueRef::Null => SqlValue::Null,
                            ValueRef::Integer(v) => SqlValue::Integer(v),
                            ValueRef::Real(v) => SqlValue::Real(v),
                            ValueRef::Text(v) => SqlValue::Text(
                                match String::from_utf8(v.to_owned()) {
                                    Ok(v) => v,
                                    Err(_) => {
                                        break 'rows_loop Err(SqlQueryExecutionError::TextValueDecodeError);
                                    }
                                },
                            ),
                            ValueRef::Blob(v) => SqlValue::Blob(v.to_owned()),
                        });
                    }
                    result_rows.push(SqlRow { columns: row_columns });
                };

                debug!(database=connection_id, "running sql exec - done");
                // error can be ignored here because that means that request has been cancelled
                let _ = msg.response.send(response_message);
            },
            SqlMessage::Batch(msg) => {
                debug!(database=connection_id, "running sql batch");
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlConnectionInitError::DatabaseBusy => {
                            // error can be ignored here because that means that request has been cancelled
                            let _ = msg.response.send(Err(SqlTaskBatchError::DatabaseBusy));
                            continue;
                        },
                        SqlConnectionInitError::UnknownError => {
                            // error can be ignored here because it means that request was cancelled
                            let _ = msg.response.send(Err(SqlTaskBatchError::UnknownError));
                            continue;
                        }
                    },
                };

                let txn = match connection.transaction() {
                    Ok(v) => v,
                    Err(err) => {
                        if is_database_busy_error(&err) {
                            // error can be ignored here because that means that request has been cancelled
                            let _ = msg.response.send(Err(SqlTaskBatchError::DatabaseBusy));
                        } else {
                            error!("unexpected sqlite error when initializing transaction: {err:?}");
                            // error can be ignored here because that means that request has been cancelled
                            let _ = msg.response.send(Err(SqlTaskBatchError::UnknownError));
                        };
                        continue;
                    }
                };


                let mut execution_result = Ok(());
                for (statement, params) in msg.queries {
                    match txn.execute(&statement, rusqlite::params_from_iter(params)) {
                        Ok(_) => {},
                        Err(rusqlite::Error::SqliteFailure(_, Some(reason))) => {
                            execution_result = Err(SqlTaskBatchError::StatementFailed { reason });
                            break;
                        },
                        Err(rusqlite::Error::SqlInputError { error: _, msg: reason, sql: _, offset: _ }) => {
                            execution_result = Err(SqlTaskBatchError::StatementFailed { reason });
                            break;
                        },
                        Err(err) => {
                            error!("unexpected sqlite error when executing transaction statement: {err:?}");
                            execution_result = Err(SqlTaskBatchError::UnknownError);
                            break;
                        },
                    }
                };

                let response = execution_result
                    .and_then(|_| txn.commit().map_err(|err| {
                        let error_code = err.sqlite_error().map(|v| v.code);
                        if error_code == Some(rusqlite::ErrorCode::DatabaseBusy) {
                            SqlTaskBatchError::DatabaseBusy
                        } else {
                            error!("unexpected sqlite error when committing transaction: {err:?}");
                            SqlTaskBatchError::UnknownError
                        }
                    }));

                debug!(database=connection_id, "running sql batch - done.");
                // error can be ignored here because it means that request was cancelled
                let _ = msg.response.send(response);
            },
            SqlMessage::Migrate(msg) => {
                debug!(database=connection_id, "running migration, busy timeout: {busy_timeout:?}");
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlConnectionInitError::DatabaseBusy => {
                            debug!("database busy when getting connection to run migration");
                            // error can be ignored here because it means that request was cancelled
                            let _ = msg.response.send(Err(SqlTaskMigrationError::DatabaseBusy));
                            continue;
                        },
                        SqlConnectionInitError::UnknownError => {
                            // error can be ignored here because it means that request was cancelled
                            let _ = msg.response.send(Err(SqlTaskMigrationError::UnknownError));
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
                                    message: message,
                                }),
                                other => {
                                    error!("unexpected rusqlite error code: {other:?}");
                                    Err(SqlTaskMigrationError::UnknownError)
                                },
                            },
                            rusqlite::Error::SqlInputError { error: _, msg, sql: _, offset: _ } => Err(SqlTaskMigrationError::SqlError {
                                message: msg,
                            }),
                            other => {
                                error!("unexpected rusqlite error: {other:?}");
                                Err(SqlTaskMigrationError::UnknownError)
                            },
                        },
                        other => {
                            error!("unexpected sqlite error: {other:?}");
                            Err(SqlTaskMigrationError::UnknownError)
                        },
                    }
                };

                debug!(database=connection_id, "running migration - done.");

                // error here means that request was cancelled and can be ignored
                let _ = msg.response.send(response);
            },
        }
    }
}

fn is_database_busy_error(err: &rusqlite::Error) -> bool {
    let error_code = match err.sqlite_error() {
        Some(v) => v.code,
        None => return false,
    };
    error_code == rusqlite::ErrorCode::DatabaseBusy || error_code == rusqlite::ErrorCode::DatabaseLocked
}

// triggered when there is a problem with sql statement, for example more values than columns when inserting
fn is_statement_error(err: &rusqlite::Error) -> Option<&String> {
    match err {
        rusqlite::Error::SqliteFailure(_, err) => err.as_ref(),
        _ => None,
    }
}
