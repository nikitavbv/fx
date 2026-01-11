use {
    std::{
        time::{SystemTime, UNIX_EPOCH},
        task::{self, Poll},
    },
    wasmer::FunctionEnvMut,
    tracing::error,
    futures::FutureExt,
    rand::TryRngCore,
    fx_api::{capnp, fx_capnp},
    fx_common::FxFutureError,
    crate::{
        runtime::{ExecutionEnv, write_memory_obj, PtrWithLen, FunctionId},
        kv::KVStorage,
        error::FunctionInvokeError,
        logs,
    },
};

// TODO: see rpc and refactor all other api calls similarly
pub(crate) mod rpc;
pub(crate) mod unsupported;

// TODO:
// - rate limiting - use governor crate and have a set of rate limits defined in FunctionDefinition
// - permissions - based on capabilities
// - metrics - counters per syscall type
// - no "fx_cloud" namespace, should be just one more binding

pub fn fx_api_handler(mut ctx: FunctionEnvMut<ExecutionEnv>, req_addr: i64, req_len: i64, output_ptr: i64) {
    let (data, mut store) = ctx.data_and_store_mut();

    let memory = data.memory.as_ref().unwrap();
    let view = memory.view(&store);
    let req_addr = req_addr as usize;
    let req_len = req_len as usize;

    let mut message_bytes = unsafe { &view.data_unchecked_mut()[req_addr..req_addr+req_len] };
    let message_reader = fx_api::capnp::serialize::read_message_from_flat_slice(&mut message_bytes, fx_api::capnp::message::ReaderOptions::default()).unwrap();
    let request = message_reader.get_root::<fx_api::fx_capnp::fx_api_call::Reader>().unwrap();
    let op = request.get_op();

    let mut response_message = capnp::message::Builder::new_default();
    let response = response_message.init_root::<fx_capnp::fx_api_call_result::Builder>();
    let mut response_op = response.init_op();

    use fx_api::fx_capnp::fx_api_call::op::{Which as Operation};
    match op.which().unwrap() {
        Operation::MetricsCounterIncrement(v) => {
            handle_metrics_counter_increment(data, v.unwrap());
            response_op.set_metrics_counter_increment(());
        },
        Operation::Rpc(v) => {
            handle_rpc(data, v.unwrap(), response_op.init_rpc());
        },
        Operation::KvGet(v) => {
            handle_kv_get(data, v.unwrap(), response_op.init_kv_get());
        },
        Operation::KvSet(v) => {
            handle_kv_set(data, v.unwrap(), response_op.init_kv_set());
        },
        Operation::SqlExec(v) => {
            handle_sql_exec(data, v.unwrap(), response_op.init_sql_exec());
        },
        Operation::SqlBatch(v) => {
            handle_sql_batch(data, v.unwrap(), response_op.init_sql_batch());
        },
        Operation::SqlMigrate(v) => {
            handle_sql_migrate(data, v.unwrap(), response_op.init_sql_migrate());
        },
        Operation::Log(v) => {
            handle_log(data, v.unwrap(), response_op.init_log());
        },
        Operation::Fetch(v) => {
            handle_fetch(data, v.unwrap(), response_op.init_fetch());
        },
        Operation::Sleep(v) => {
            handle_sleep(data, v.unwrap(), response_op.init_sleep());
        },
        Operation::Random(v) => {
            handle_random(v.unwrap(), response_op.init_random());
        },
        Operation::Time(_) => {
            handle_time(response_op.init_time());
        },
        Operation::FuturePoll(v) => {
            handle_future_poll(data, v.unwrap(), response_op.init_future_poll());
        },
        Operation::FutureDrop(v) => {
            handle_future_drop(data, v.unwrap(), response_op.init_future_drop());
        },
        Operation::StreamExport(v) => {
            handle_stream_export(data, v.unwrap(), response_op.init_stream_export());
        },
        Operation::StreamPollNext(v) => {
            handle_stream_poll_next(data, v.unwrap(), response_op.init_stream_poll_next());
        }
    };

    let response_size = capnp::serialize::compute_serialized_size_in_words(&response_message) * 8;
    let ptr = data.client_malloc().call(&mut store, &[wasmer::Value::I64(response_size as i64)]).unwrap()[0].i64().unwrap() as usize;

    unsafe {
        capnp::serialize::write_message(&mut memory.view(&store).data_unchecked_mut()[ptr..ptr+response_size], &response_message).unwrap();
    }

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr: ptr as i64, len: response_size as i64 });
}

fn handle_metrics_counter_increment(data: &ExecutionEnv, counter_increment_request: fx_capnp::metrics_counter_increment_request::Reader) {
    let result = data.engine.metrics.function_metrics.counter_increment(
        &data.function_id,
        counter_increment_request.get_counter_name().unwrap().to_str().unwrap(),
        counter_increment_request.get_tags().unwrap()
            .into_iter()
            .map(|v| (v.get_name().unwrap().to_string().unwrap(), v.get_value().unwrap().to_string().unwrap()))
            .collect(),
        counter_increment_request.get_delta()
    );

    if let Err(err) = result {
        error!("failed to increment counter: {err:?}");
    }
}

fn handle_rpc(data: &ExecutionEnv, rpc_request: fx_capnp::rpc_call_request::Reader, rpc_response: fx_capnp::rpc_call_response::Builder) {
    let mut rpc_response = rpc_response.init_response();

    let function_id = FunctionId::new(rpc_request.get_function_id().unwrap().to_string().unwrap());
    if data.rpc.get(&function_id.as_string()).is_none() {
        rpc_response.set_binding_not_found(());
    };

    let method_name = rpc_request.get_method_name().unwrap().to_str().unwrap();
    let argument = rpc_request.get_argument().unwrap();

    let engine = data.engine.clone();
    let response_future = match engine.clone().invoke_service_raw(engine.clone(), function_id.clone(), method_name.to_owned(), argument.to_vec()) {
        Ok(response_future) => response_future.map(|v| v
            .map(|v| v.0)
            .map_err(|err| FxFutureError::RpcError {
                reason: err.to_string(),
            })
        ).boxed(),
        Err(err) => {
            match err {
                FunctionInvokeError::RuntimeError(runtime_error) => {
                    error!("failed to execute rpc api because of internal error: {runtime_error:?}");
                    rpc_response.set_runtime_error(());
                    return;
                }
            }
        },
    };
    let response_future = match data.engine.futures_pool.push(response_future.boxed()) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to push future to futures arena: {err:?}");
            rpc_response.set_runtime_error(());
            return;
        }
    };

    engine.metrics.function_fx_api_calls.with_label_values(&[data.function_id.as_string().as_str(), "rpc"]).inc();

    rpc_response.set_future_id(response_future.0);
}

fn handle_kv_get(data: &ExecutionEnv, kv_get_request: fx_capnp::kv_get_request::Reader, kv_get_response: fx_capnp::kv_get_response::Builder) {
    let mut kv_get_response = kv_get_response.init_response();

    let binding = kv_get_request.get_binding_id().unwrap().to_str().unwrap();
    let storage = match data.storage.get(binding) {
        Some(v) => v,
        None => {
            kv_get_response.set_binding_not_found(());
            return;
        }
    };

    let key = kv_get_request.get_key().unwrap();
    let value = storage.get(key).unwrap();
    let value = match value {
        Some(v) => v,
        None => {
            kv_get_response.set_key_not_found(());
            return;
        }
    };

    kv_get_response.set_value(&value);

    data.engine.metrics.function_fx_api_calls.with_label_values(&[data.function_id.as_string().as_str(), "kv::get"]).inc();
}

fn handle_kv_set(data: &ExecutionEnv, kv_set_request: fx_capnp::kv_set_request::Reader, kv_set_response: fx_capnp::kv_set_response::Builder) {
    let mut kv_set_response = kv_set_response.init_response();

    let binding = kv_set_request.get_binding_id().unwrap().to_str().unwrap();
    let storage = match data.storage.get(binding) {
        Some(v) => v,
        None => {
            kv_set_response.set_binding_not_found(());
            return;
        }
    };

    storage.set(kv_set_request.get_key().unwrap(), kv_set_request.get_value().unwrap()).unwrap();

    kv_set_response.set_ok(());

    data.engine.metrics.function_fx_api_calls.with_label_values(&[data.function_id.as_string().as_str(), "kv::set"]).inc();
}

fn handle_sql_exec(data: &ExecutionEnv, sql_exec_request: fx_capnp::sql_exec_request::Reader, sql_exec_response: fx_capnp::sql_exec_response::Builder) {
    let mut sql_exec_response = sql_exec_response.init_response();

    let binding = sql_exec_request.get_database().unwrap().to_string().unwrap();
    let database = match data.sql.get(&binding) {
        Some(v) => v,
        None => {
            sql_exec_response.set_binding_not_found(());
            return;
        }
    };

    let request_query = sql_exec_request.get_query().unwrap();
    let query = sql_query_from_reader(request_query);

    let result = match database.exec(query) {
        Ok(v) => v,
        Err(err) => {
            let mut error_response = sql_exec_response.init_sql_error();
            error_response.set_description(err.to_string());
            return;
        }
    };

    let mut response_rows = sql_exec_response.init_rows(result.rows.len() as u32);
    for (index, result_row) in result.rows.into_iter().enumerate() {
        let mut response_row_columns = response_rows.reborrow().get(index as u32).init_columns(result_row.columns.len() as u32);
        for (column_index, value) in result_row.columns.into_iter().enumerate() {
            let mut response_value = response_row_columns.reborrow().get(column_index as u32).init_value();
            match value {
                crate::sql::Value::Null => response_value.set_null(()),
                crate::sql::Value::Integer(v) => response_value.set_integer(v),
                crate::sql::Value::Real(v) => response_value.set_real(v),
                crate::sql::Value::Text(v) => response_value.set_text(v),
                crate::sql::Value::Blob(v) => response_value.set_blob(&v),
            }
        }
    }

    data.engine.metrics.function_fx_api_calls.with_label_values(&[data.function_id.as_string().as_str(), "sql::exec"]).inc();
}

fn handle_sql_batch(data: &ExecutionEnv, sql_batch_request: fx_capnp::sql_batch_request::Reader, sql_batch_response: fx_capnp::sql_batch_response::Builder) {
    let mut sql_batch_response = sql_batch_response.init_response();

    let binding = sql_batch_request.get_database().unwrap().to_string().unwrap();
    let database = match data.sql.get(&binding) {
        Some(v) => v,
        None => {
            sql_batch_response.set_binding_not_found(());
            return;
        }
    };

    let queries = sql_batch_request.get_queries()
        .unwrap()
        .into_iter()
        .map(sql_query_from_reader)
        .collect::<Vec<_>>();

    match database.batch(queries) {
        Ok(_) => {
            sql_batch_response.set_ok(());
        },
        Err(err) => {
            sql_batch_response.init_sql_error().set_description(err.to_string());
        }
    }
}

fn sql_query_from_reader(request_query: fx_capnp::sql_query::Reader<'_>) -> crate::sql::Query {
    let mut query = crate::sql::Query::new(request_query.get_statement().unwrap().to_string().unwrap());
    for param in request_query.get_params().unwrap().into_iter() {
        use fx_capnp::sql_value::value::{Which as SqlValue};

        query = query.with_param(match param.get_value().which().unwrap() {
            SqlValue::Null(_) => crate::sql::Value::Null,
            SqlValue::Integer(v) => crate::sql::Value::Integer(v),
            SqlValue::Real(v) => crate::sql::Value::Real(v),
            SqlValue::Text(v) => crate::sql::Value::Text(v.unwrap().to_string().unwrap()),
            SqlValue::Blob(v) => crate::sql::Value::Blob(v.unwrap().to_vec()),
        })
    }
    query
}

fn handle_sql_migrate(data: &ExecutionEnv, sql_migrate_request: fx_capnp::sql_migrate_request::Reader, sql_migrate_response: fx_capnp::sql_migrate_response::Builder) {
    let mut sql_migrate_response = sql_migrate_response.init_response();

    let binding = sql_migrate_request.get_database().unwrap().to_string().unwrap();
    let database = match data.sql.get(&binding) {
        Some(v) => v,
        None => {
            sql_migrate_response.set_binding_not_found(());
            return;
        }
    };

    let migrations = sql_migrate_request.get_migrations().unwrap().into_iter().map(|v| v.unwrap().to_string().unwrap()).collect();
    match database.migrate(migrations) {
        Ok(_) => {
            sql_migrate_response.set_ok(());
        },
        Err(err) => {
            sql_migrate_response.init_sql_error().set_description(err.to_string());
        }
    }
}

fn handle_log(data: &ExecutionEnv, log_request: fx_capnp::log_request::Reader, _log_response: fx_capnp::log_response::Builder) {
    data.engine.log(logs::LogMessage::new(
        logs::LogSource::function(&data.function_id),
        match log_request.get_event_type().unwrap() {
            fx_capnp::EventType::Begin => logs::LogEventType::Begin,
            fx_capnp::EventType::End => logs::LogEventType::End,
            fx_capnp::EventType::Instant => logs::LogEventType::Instant,
        },
        match log_request.get_level().unwrap() {
            fx_capnp::LogLevel::Trace => logs::LogLevel::Trace,
            fx_capnp::LogLevel::Debug => logs::LogLevel::Debug,
            fx_capnp::LogLevel::Info => logs::LogLevel::Info,
            fx_capnp::LogLevel::Warn => logs::LogLevel::Warn,
            fx_capnp::LogLevel::Error => logs::LogLevel::Error,
        },
        log_request.get_fields().unwrap()
            .into_iter()
            .map(|v| (v.get_name().unwrap().to_string().unwrap(), v.get_value().unwrap().to_string().unwrap()))
            .collect()
    ).into());
}

fn handle_fetch(data: &ExecutionEnv, fetch_request: fx_capnp::fetch_request::Reader, fetch_response: fx_capnp::fetch_response::Builder) {
    let mut fetch_response = fetch_response.init_response();

    let method = match fetch_request.get_method() {
        Ok(v) => v,
        Err(err) => {
            fetch_response.set_fetch_error(format!("failed to read http method from fetch_request: unknown http method: {err:?}"));
            return;
        }
    };

    let url = match fetch_request.get_url() {
        Ok(v) => v,
        Err(err) => {
            fetch_response.set_fetch_error(format!("failed to read target url from fetch_request: {err:?}"));
            return;
        }
    };

    let url = match url.to_str() {
        Ok(v) => v,
        Err(err) => {
            fetch_response.set_fetch_error(format!("failed to decode target url as string: {err:?}"));
            return;
        }
    };

    let request = data.fetch_client
        .request(
            match method {
                fx_capnp::HttpMethod::Get => http::Method::GET,
                fx_capnp::HttpMethod::Post => http::Method::POST,
                fx_capnp::HttpMethod::Put => http::Method::PUT,
                fx_capnp::HttpMethod::Patch => http::Method::PATCH,
                fx_capnp::HttpMethod::Delete => http::Method::DELETE,
            },
            url
        )
        .headers({
            let mut headers = http::HeaderMap::new();

            for header in fetch_request.get_headers().unwrap().into_iter() {
                headers.append(
                    http::HeaderName::from_bytes(header.get_name().unwrap().as_bytes()).unwrap(),
                    header.get_value().unwrap().to_string().unwrap().parse().unwrap()
                );
            }

            headers
        });

    let request = if let Ok(body) = fetch_request.get_body() {
        let body = fx_common::FxStream { index: body.get_id() as i64 };
        let stream = data.engine.streams_pool.read(data.engine.clone(), &body);

        match stream {
            Ok(Some(stream)) => request.body(reqwest::Body::wrap_stream(stream)),
            Ok(None) => request,
            Err(err) => {
                fetch_response.set_fetch_error(format!("failed to read stream: {err:?}"));
                return;
            }
        }
    } else {
        request
    };

    let request_future = async move {
        use futures::TryFutureExt;

        request.send()
            .and_then(|response| async {
                Ok(rmp_serde::to_vec(&fx_common::HttpResponse {
                    status: response.status(),
                    headers: response.headers().clone(),
                    body: response.bytes().await.unwrap().to_vec(),
                }).unwrap())
            })
            .await
            .map_err(|err| FxFutureError::FetchError {
                reason: format!("request failed: {err:?}"),
            })
    }.boxed();

    let index = data.engine.futures_pool.push(request_future).unwrap();
    fetch_response.set_future_id(index.0);
}

fn handle_sleep(data: &ExecutionEnv, sleep_request: fx_capnp::sleep_request::Reader, sleep_response: fx_capnp::sleep_response::Builder) {
    let mut response = sleep_response.init_response();

    let sleep = tokio::time::sleep(std::time::Duration::from_millis(sleep_request.get_millis()));
    match data.engine.futures_pool.push(sleep.map(|_| Ok(Vec::new())).boxed()) {
        Ok(v) => {
            response.set_future_id(v.0);
        },
        Err(err) => {
            response.set_sleep_error(err.to_string());
        }
    }
}

fn handle_random(random_request: fx_capnp::random_request::Reader, mut random_response: fx_capnp::random_response::Builder) {
    let mut random_data = vec![0; random_request.get_length() as usize];
    rand::rngs::OsRng.try_fill_bytes(&mut random_data).unwrap();
    random_response.set_data(&random_data);
}

fn handle_time(mut time_response: fx_capnp::time_response::Builder) {
    time_response.set_timestamp(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64)
}

fn handle_future_poll(data: &ExecutionEnv, future_poll_request: fx_capnp::future_poll_request::Reader, future_poll_response: fx_capnp::future_poll_response::Builder) {
    use futures::task::Poll;

    let mut response = future_poll_response.init_response();

    let result = data.engine.futures_pool.poll(&crate::futures::HostPoolIndex(future_poll_request.get_future_id()), &mut futures::task::Context::from_waker(data.futures_waker.as_ref().unwrap()));

    match result {
        Poll::Pending => {
            response.set_pending(());
        },
        Poll::Ready(res) => {
            match res {
                Ok(v) => response.set_result(&v),
                Err(error) => {
                    let mut response_error = response.init_error().init_error();
                    match error {
                    }
                },
            }
        }
    }
}

fn handle_future_drop(data: &ExecutionEnv, future_drop_request: fx_capnp::future_drop_request::Reader, _future_drop_response: fx_capnp::future_drop_response::Builder) {
    data.engine.futures_pool.remove(&crate::futures::HostPoolIndex(future_drop_request.get_future_id()));
}

fn handle_stream_export(data: &ExecutionEnv, _stream_export_request: fx_capnp::stream_export_request::Reader, stream_export_response: fx_capnp::stream_export_response::Builder) {
    let mut response = stream_export_response.init_response();

    match data.engine.streams_pool.push_function_stream(data.function_id.clone()) {
        Ok(v) => {
            response.set_stream_id(v.0);
        },
        Err(err) => {
            response.set_error(err.to_string());
        }
    }
}

fn handle_stream_poll_next(data: &ExecutionEnv, stream_poll_next_request: fx_capnp::stream_poll_next_request::Reader, stream_poll_next_response: fx_capnp::stream_poll_next_response::Builder) {
    let mut response = stream_poll_next_response.init_response();

    let result = data.engine.streams_pool.poll_next(
        data.engine.clone(),
        &crate::streams::HostPoolIndex(stream_poll_next_request.get_stream_id()),
        &mut task::Context::from_waker(data.futures_waker.as_ref().unwrap())
    );

    match result {
        Poll::Pending => {
            response.set_pending(());
        },
        Poll::Ready(Some(v)) => {
            let mut item = response.init_ready().init_item();
            match v {
                Ok(v) => {
                    item.set_result(&v);
                },
                Err(err) => {
                    item.set_error(err.to_string());
                }
            }
        },
        Poll::Ready(None) => {
            response.set_finished(());
        }
    }
}
