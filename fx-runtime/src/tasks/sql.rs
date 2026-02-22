#[derive(Debug)]
enum SqlMessage {
    Exec(SqlExecMessage),
    Migrate(SqlMigrateMessage),
}

#[derive(Debug)]
struct SqlExecMessage {
    binding: SqlBindingConfig,
    statement: String,
    params: Vec<SqlValue>,
    response: oneshot::Sender<SqlQueryResult>,
}

#[derive(Debug)]
enum SqlQueryResult {
    Ok(Vec<SqlRow>),
    Error(SqlQueryExecutionError),
}

#[derive(Debug, Error)]
enum SqlQueryExecutionError {
    #[error("database is locked")]
    DatabaseBusy,
}

#[derive(Debug)]
enum SqlMigrationResult {
    Ok(()),
    Error(SqlMigrationError),
}

#[derive(Debug, Error)]
enum SqlMigrationError {
    #[error("database is locked")]
    DatabaseBusy,
}

#[derive(Debug)]
struct SqlMigrateMessage {
    binding: SqlBindingConfig,
    migrations: Vec<String>,
    response: oneshot::Sender<SqlMigrationResult>,
}

fn run_sql_task(sql_rx: flume::Receiver<SqlMessage>) {
    use rusqlite::types::ValueRef;

    let mut connections = HashMap::<String, rusqlite::Connection>::new();

    while let Ok(msg) = sql_rx.recv() {
        let binding = match &msg {
            SqlMessage::Exec(v) => &v.binding,
            SqlMessage::Migrate(v) => &v.binding,
        };

        let connection = match connections.entry(binding.connection_id.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let connection = match &binding.location {
                    SqlBindingConfigLocation::InMemory(v) => rusqlite::Connection::open_with_flags(
                        format!("file:{v}"),
                        rusqlite::OpenFlags::default()
                            .union(rusqlite::OpenFlags::SQLITE_OPEN_MEMORY)
                            .union(rusqlite::OpenFlags::SQLITE_OPEN_SHARED_CACHE)
                    ).unwrap(),
                    SqlBindingConfigLocation::Path(v) => rusqlite::Connection::open(v).unwrap(),
                };
                if let Some(busy_timeout) = binding.busy_timeout {
                    connection.busy_timeout(busy_timeout).unwrap();
                }
                if let Err(err) = connection.pragma_update(None, "journal_mode", "WAL") {
                    if err.sqlite_error().unwrap().code == rusqlite::ErrorCode::DatabaseBusy {
                        Err(SqlQueryExecutionError::DatabaseBusy)
                    } else {
                        panic!("unexpected sqlite error: {err:?}")
                    }
                } else {
                    connection.pragma_update(None, "synchronous", "NORMAL").unwrap();
                    Ok(entry.insert(connection))
                }
            }
        };

        match msg {
            SqlMessage::Exec(msg) => {
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlQueryExecutionError::DatabaseBusy => {
                            msg.response.send(SqlQueryResult::Error(SqlQueryExecutionError::DatabaseBusy)).unwrap();
                            continue;
                        }
                    }
                };

                let mut stmt = connection.prepare(&msg.statement).unwrap();
                let result_columns = stmt.column_count();

                let mut rows = stmt.query(rusqlite::params_from_iter(msg.params.into_iter())).unwrap();

                let mut result_rows = Vec::new();
                let response_message = loop {
                    let row = rows.next();
                    let row = match row {
                        Ok(v) => v,
                        Err(err) => {
                            if err.sqlite_error().unwrap().code == rusqlite::ErrorCode::DatabaseBusy {
                                break SqlQueryResult::Error(SqlQueryExecutionError::DatabaseBusy);
                            } else {
                                panic!("unexpected sqlite error: {err:?}")
                            }
                        }
                    };
                    let row = match row {
                        Some(v) => v,
                        None => break SqlQueryResult::Ok(result_rows),
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

                msg.response.send(response_message).unwrap();
            },
            SqlMessage::Migrate(msg) => {
                let connection = match connection {
                    Ok(v) => v,
                    Err(err) => match err {
                        SqlQueryExecutionError::DatabaseBusy => {
                            msg.response.send(SqlMigrationResult::Error(SqlMigrationError::DatabaseBusy)).unwrap();
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
                    Ok(_) => SqlMigrationResult::Ok(()),
                    Err(err) => match err {
                        rusqlite_migration::Error::RusqliteError { query: _, err } => if err.sqlite_error().unwrap().code == rusqlite::ErrorCode::DatabaseBusy {
                            SqlMigrationResult::Error(SqlMigrationError::DatabaseBusy)
                        } else {
                            panic!("unexpected sqlite error: {err:?}");
                        },
                        other => panic!("unexpected sqlite error: {other:?}"),
                    }
                };

                msg.response.send(response).unwrap();
            },
        }
    }
}
