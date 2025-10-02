use {
    wasmer::{FunctionEnvMut, Value},
    fx_common::{FxSqlError, DatabaseSqlQuery, SqlValue, SqlResult, SqlResultRow},
    crate::{sql, runtime::{ExecutionEnv, decode_memory, write_memory, write_memory_obj, PtrWithLen}},
};

pub fn handle_sql_exec(mut ctx: FunctionEnvMut<ExecutionEnv>, query_addr: i64, query_len: i64, output_ptr: i64) {
    let result = decode_memory(&ctx, query_addr, query_len)
        .map_err(|err| FxSqlError::SerializationError { reason: format!("failed to decode request: {err:?}") })
        .and_then(|request: DatabaseSqlQuery| {
            let mut query = sql::Query::new(request.query.stmt);
            for param in request.query.params {
                query = query.with_param(match param {
                    SqlValue::Null => sql::Value::Null,
                    SqlValue::Integer(v) => sql::Value::Integer(v),
                    SqlValue::Real(v) => sql::Value::Real(v),
                    SqlValue::Text(v) => sql::Value::Text(v),
                    SqlValue::Blob(v) => sql::Value::Blob(v),
                });
            }

            ctx.data().sql.get(&request.database)
                .as_ref()
                .ok_or(FxSqlError::BindingNotExists)
                .and_then(|database| database.exec(query).map_err(|err| FxSqlError::QueryFailed { reason: err.to_string() }))
                .map(|result| SqlResult {
                    rows: result.rows.into_iter()
                        .map(|row| SqlResultRow {
                            columns: row.columns.into_iter()
                                .map(|value| match value {
                                    sql::Value::Null => SqlValue::Null,
                                    sql::Value::Integer(v) => SqlValue::Integer(v),
                                    sql::Value::Real(v) => SqlValue::Real(v),
                                    sql::Value::Text(v) => SqlValue::Text(v),
                                    sql::Value::Blob(v) => SqlValue::Blob(v),
                                })
                                .collect(),
                        })
                        .collect(),
                })
        });
    let result = rmp_serde::to_vec(&result).unwrap();

    let (data, mut store) = ctx.data_and_store_mut();

    let len = result.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &result);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });
}
