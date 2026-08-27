pub(crate) use fx_types::{
    capnp,
    abi_log_capnp,
    abi_sql_capnp,
    abi_http_capnp,
    abi_metrics_capnp,
    abi_blob_capnp,
    abi_kv_capnp,
};

use {
    std::{task::Poll, time::{SystemTime, UNIX_EPOCH}, str::FromStr, collections::HashMap, future::ready},
    tokio::{sync::oneshot, time::Duration},
    tracing::{debug, error, warn},
    http::Method,
    http_body_util::{BodyStream, BodyExt},
    wasmtime::{AsContext, AsContextMut},
    futures::{FutureExt, StreamExt, TryStreamExt},
    rand::TryRngCore,
    send_wrapper::SendWrapper,
    zerocopy::IntoBytes,
    tower::Service,
    fx_types::abi::{
        ResourceMoveFromHostResult,
        UnitFuturePollResult,
        SqlQueryResultFuturePollResult,
        SqlQueryResultSerializeResult,
        SqlBatchResultFuturePollResult,
        SqlBatchResultSerializeResult,
        SqlMigrationResultSerializeResult,
        FetchResultFuturePollResult,
        FetchResultSerializeResult,
        HttpBodyPollFrameResult,
        HttpFrameSerializeResult,
        HttpFrameSerializeResultCode,
        AsyncResourcePollResult,
        BlobPutResultSerializeResult,
        BlobPutResultSerializeResultCode,
        BlobGetResultSerializeResult,
        BlobGetResultSerializeResultCode,
        BlobDeleteResultSerializeResult,
        BlobDeleteResultSerializeResultCode,
        ResourceSerializeResult,
        KvSubscriptionStreamPollResult,
        EnvGetResult,
        EnvLenResult,
        EnvLenResultCode,
        MetricsCounterRegisterResult,
        MetricsCounterRegisterResultCode,
    },
    crate::{
        function::instance::FunctionInstanceState,
        resources::{
            FunctionResourceId,
            resource::{
                ResourceTable,
                FunctionResources,
                FetchRequestHeaderResourceKey,
                UnitFutureResourceKey,
                BlobGetResponseFutureResourceKey,
            },
        },
        effects::{
            logs::{LogMessageEvent, LogSource, LogEventType, LogEventLevel, EventFieldValue},
            sql::{SqlValue, SqlBatchError, SqlMigrationError, SqlQueryError},
            blob::{BlobPutError, BlobGetError, BlobDeleteError},
            fetch::{FetchResultWithBodyResource, FetchResultError, HttpStreamError},
            metrics::{MetricKey, MetricId},
            kv::{KvGetHandlerError, KvDelexRequest, KvDelexHandlerError, KvSubscriptionResource, KvPublishRequest, KvPublishHandlerError, KvSubscriptionHandlerError},
        },
        tasks::{
            sql::{SqlMessage, SqlExecMessage, SqlBatchMessage, SqlMigrateMessage},
            kv::{KvMessage, KvOperation},
            blob::BlobMessage,
        },
        triggers::http::{HttpBody, HttpBodyInner, FunctionStreamReader},
    },
};

pub(crate) mod function_memory;

pub(super) fn fx_log_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: u64, req_len: u64) {
    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to handle log message, failed to access function memory: {err:?}");
            return;
        },
    };
    let context = caller.as_context();
    let memory = memory.view(&context);

    let mut message_bytes = match memory.slice(req_addr, req_len) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to handle log message, failed to read function memory: {err:?}");
            return;
        },
    };
    let message_reader = match capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to handle log message, failed to read message: {err:?}");
            return;
        }
    };
    let message = match message_reader.get_root::<abi_log_capnp::log_message::Reader>() {
        Ok(v) => v,
        Err(err) => {
            error!("failed to handle log message, failed to get message root: {err:?}");
            return;
        }
    };

    let message: LogMessageEvent = LogMessageEvent::new(
        LogSource::function(&caller.data().function_id),
        message.get_event_type().map(|v| match v {
            abi_log_capnp::EventType::Begin => LogEventType::Begin,
            abi_log_capnp::EventType::End => LogEventType::End,
            abi_log_capnp::EventType::Instant => LogEventType::Instant,
        }).unwrap_or(LogEventType::Instant),
        message.get_level().map(|v| match v {
            abi_log_capnp::LogLevel::Trace => LogEventLevel::Trace,
            abi_log_capnp::LogLevel::Debug => LogEventLevel::Debug,
            abi_log_capnp::LogLevel::Info => LogEventLevel::Info,
            abi_log_capnp::LogLevel::Warn => LogEventLevel::Warn,
            abi_log_capnp::LogLevel::Error => LogEventLevel::Error,
        }).unwrap_or(LogEventLevel::Info),
        match message.get_fields() {
            Ok(fields) => fields
                .into_iter()
                .filter_map(|v| {
                    let name = match v.get_name().ok()?.to_string() {
                        Ok(v) => v,
                        Err(err) => {
                            error!("failed to decode log message field name: {err:?}");
                            return None;
                        }
                    };
                    let value = match v.get_value().ok()?.to_string() {
                        Ok(v) => v,
                        Err(err) => {
                            error!("failed to decode log message field value: {err:?}");
                            return None;
                        }
                    };

                    Some((name, EventFieldValue::Text(value)))
                })
                .collect(),
            Err(_) => HashMap::new(),
        }
    ).into();

    if caller.data().runtime_services.logger.send(message).is_err() {
        warn!("failed to write log message to logger: log channel is closed.");
    }
}

pub(super) fn fx_fetch_request_header_serialize_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) -> u64 {
    let resource_set = &mut caller.data_mut().resource_set;
    let fetch_request_header = resource_set.fetch_request_headers.remove(FetchRequestHeaderResourceKey::from(resource_id)).unwrap();

    let mut message = capnp::message::Builder::new_default();
    let mut resource = message.init_root::<abi_http_capnp::http_request::Builder>();

    resource.set_uri(fetch_request_header.uri().to_string());
    resource.set_method(match fetch_request_header.method() {
        &hyper::Method::GET => abi_http_capnp::HttpMethod::Get,
        &hyper::Method::POST => abi_http_capnp::HttpMethod::Post,
        &hyper::Method::PUT => abi_http_capnp::HttpMethod::Put,
        &hyper::Method::PATCH => abi_http_capnp::HttpMethod::Patch,
        &hyper::Method::DELETE => abi_http_capnp::HttpMethod::Delete,
        &hyper::Method::OPTIONS => abi_http_capnp::HttpMethod::Options,
        &hyper::Method::HEAD => abi_http_capnp::HttpMethod::Head,
        &hyper::Method::CONNECT => abi_http_capnp::HttpMethod::Connect,
        &hyper::Method::TRACE => abi_http_capnp::HttpMethod::Trace,
        other => panic!("http method not supported: {other:?}"),
    });

    let mut request_headers = resource.reborrow().init_headers(fetch_request_header.headers().len() as u32);
    for (index, (header_name, header_value)) in fetch_request_header.headers().iter().enumerate() {
        let mut request_header = request_headers.reborrow().get(index as u32);
        request_header.set_name(header_name.as_str());
        request_header.set_value(header_value.to_str().unwrap());
    }

    let mut resource_body = resource.init_body().init_body();
    match fetch_request_header.into_body() {
        None => resource_body.set_empty(()),
        Some(resource_id) => resource_body.set_host_resource(resource_id.into()),
    }

    resource_set.bytes.insert(capnp::serialize::write_message_to_words(&message)).into()
}

pub(super) fn fx_bytes_len_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) -> u64 {
    caller.data_mut().resource_set.bytes.get(resource_id.into()).unwrap().len() as u64
}

pub(super) fn fx_bytes_move_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) -> u64 {
    let bytes = caller.data_mut().resource_set.bytes.remove(resource_id.into()).unwrap();

    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(err) => match err {
            function_memory::FunctionMemoryError::MemoryNotFound
            | function_memory::FunctionMemoryError::MemoryNotMemory => return ResourceMoveFromHostResult::FailedToAccessMemory as u64,
        }
    };
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);

    (match view.copy_from_slice(ptr, bytes.len() as u64, &bytes) {
        Ok(_) => ResourceMoveFromHostResult::Ok,
        Err(err) => match err {
            function_memory::FunctionMemoryAccessError::OutOfBounds => ResourceMoveFromHostResult::ArgumentOutOfMemoryBounds,
        }
    }) as u64
}

pub(super) fn fx_unit_future_poll(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let result = {
        let key: UnitFutureResourceKey = resource_id.into();
        let function_state = caller.data_mut();

        let mut cx = std::task::Context::from_waker(function_state.waker.as_ref().unwrap());
        let future = function_state.resource_set.unit_futures.get_mut(key.clone()).unwrap();
        match future.poll_unpin(&mut cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => {
                let _ = function_state.resource_set.unit_futures.remove(key).unwrap();
                Poll::Ready(())
            }
        }
    };

    let result = match result {
        Poll::Pending => UnitFuturePollResult { tag: 1 },
        Poll::Ready(_) => UnitFuturePollResult { tag: 0 },
    };
    let result = result.as_bytes();

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);
    view.copy_from_slice(result_addr, result.len() as u64, result).unwrap();

    0
}

pub(super) fn fx_kv_subscription_stream_poll_next(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let data = caller.data_mut();
    let subscription_stream = data.resource_set.kv_subscriptions.get_mut(resource_id.into()).unwrap();

    let result = match subscription_stream {
        Ok(subscription_stream) => {
            let waker = data.waker.clone().unwrap();
            let mut cx = std::task::Context::from_waker(&waker);

            let result = subscription_stream.poll_next_unpin(&mut cx);

            match result {
                Poll::Ready(Some(v)) => KvSubscriptionStreamPollResult {
                    tag: 1,
                    _pad: Default::default(),
                    resolved_resource_id: data.resource_set.bytes.insert(v).into(),
                },
                Poll::Ready(None) => KvSubscriptionStreamPollResult {
                    tag: 0,
                    ..Default::default()
                },
                Poll::Pending => KvSubscriptionStreamPollResult {
                    tag: 2,
                    ..Default::default()
                },
            }
        },
        Err(KvSubscriptionHandlerError::RuntimeShutdown) => KvSubscriptionStreamPollResult { tag: 3, ..Default::default() },
        Err(KvSubscriptionHandlerError::BindingNotFound) => KvSubscriptionStreamPollResult { tag: 4, ..Default::default() },
        Err(KvSubscriptionHandlerError::BadRequest) => KvSubscriptionStreamPollResult { tag: 5, ..Default::default() },
        Err(KvSubscriptionHandlerError::FailedToReadRequest) => KvSubscriptionStreamPollResult { tag: 6, ..Default::default() },
    };

    write_result(&mut caller, result_addr, result);

    0
}

pub(super) fn fx_kv_publish_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let kv_publish_response = caller.data_mut().resource_set.kv_publish_results.remove(resource_id.into()).unwrap();

    let mut message = capnp::message::Builder::new_default();
    let response = message.init_root::<abi_kv_capnp::kv_publish_result::Builder>();
    let mut response = response.init_result();

    match kv_publish_response {
        Ok(()) => response.set_ok(()),
        Err(KvPublishHandlerError::RuntimeShutdown) => response.set_runtime_shutdown(()),
        Err(KvPublishHandlerError::BindingNotFound) => response.set_binding_not_found(()),
        Err(KvPublishHandlerError::BadRequest) => response.set_bad_request(()),
        Err(KvPublishHandlerError::FailedToReadRequest) => response.set_failed_to_read_request(()),
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);

    write_result(&mut caller, result_addr, ResourceSerializeResult {
        bytes_resource_id: bytes_resource_id.into(),
        bytes_length: bytes_length as u64,
    });

    0
}

pub(super) fn fx_sql_query_result_future_poll(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let result = resource_poll(
        &mut caller,
        |s| &mut s.sql_query_result_futures,
        |s| &mut s.sql_query_results,
        resource_id
    );

    write_result(&mut caller, result_addr, match result {
        Poll::Pending => SqlQueryResultFuturePollResult {
            tag: 1,
            _pad: Default::default(),
            sql_query_result_resource_id: 0,
        },
        Poll::Ready(sql_query_result_resource_id) => SqlQueryResultFuturePollResult {
            tag: 0,
            _pad: Default::default(),
            sql_query_result_resource_id: sql_query_result_resource_id.into(),
        },
    });

    0
}

pub(super) fn fx_sql_query_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let sql_query_result = caller.data_mut().resource_set.sql_query_results.remove(resource_id.into()).unwrap();

    let mut message = capnp::message::Builder::new_default();
    let sql_exec_response = message.init_root::<abi_sql_capnp::sql_exec_result::Builder>();
    let sql_exec_response = sql_exec_response.init_result();

    match sql_query_result {
        Ok(rows) => {
            let mut response_rows = sql_exec_response.init_rows(rows.len() as u32);
            for (index, result_row) in rows.into_iter().enumerate() {
                let mut response_row_columns = response_rows.reborrow().get(index as u32).init_columns(result_row.columns.len() as u32);
                for (column_index, value) in result_row.columns.into_iter().enumerate() {
                    let mut response_value = response_row_columns.reborrow().get(column_index as u32).init_value();
                    match value {
                        SqlValue::Null => response_value.set_null(()),
                        SqlValue::Integer(v) => response_value.set_integer(v),
                        SqlValue::Real(v) => response_value.set_real(v),
                        SqlValue::Text(v) => response_value.set_text(v),
                        SqlValue::Blob(v) => response_value.set_blob(&v),
                    }
                }
            }
        },
        Err(err) => {
            let mut response_error = sql_exec_response.init_error().init_error();
            match err {
                SqlQueryError::BindingNotFound => response_error.set_binding_not_found(()),
                SqlQueryError::DatabaseBusy => response_error.set_database_busy(()),
                SqlQueryError::RuntimeShutdown => response_error.set_runtime_shutdown(()),
                SqlQueryError::StatementError(reason) => response_error.set_statement_error(reason),
                SqlQueryError::TextValueDecodeError => response_error.set_text_value_decode_error(()),
                SqlQueryError::UnknownError => response_error.set_unknown_error(()),
                SqlQueryError::RuntimeError => response_error.set_runtime_error(()),
            }
        }
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);
    let result = SqlQueryResultSerializeResult {
        bytes_resource_id: bytes_resource_id.into(),
        bytes_length: bytes_length as u64,
    };
    let result = result.as_bytes();

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);
    view.copy_from_slice(result_addr, result.len() as u64, result).unwrap();

    0
}

pub(super) fn fx_sql_batch_result_future_poll(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let result = resource_poll(
        &mut caller,
        |s| &mut s.sql_batch_result_futures,
        |s| &mut s.sql_batch_results,
        resource_id
    );

    write_result(&mut caller, result_addr, match result {
        Poll::Pending => SqlBatchResultFuturePollResult {
            tag: 1,
            _pad: Default::default(),
            sql_batch_result_resource_id: 0,
        },
        Poll::Ready(sql_batch_result_resource_id) => SqlBatchResultFuturePollResult {
            tag: 0,
            _pad: Default::default(),
            sql_batch_result_resource_id: sql_batch_result_resource_id.into(),
        },
    });

    0
}

pub(super) fn fx_sql_batch_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let resource = caller.data_mut().resource_set.sql_batch_results.remove(resource_id.into()).unwrap();

    let mut message = capnp::message::Builder::new_default();

    let sql_batch_result = message.init_root::<abi_sql_capnp::sql_batch_result::Builder>();
    let mut sql_batch_result = sql_batch_result.init_result();

    match resource {
        Ok(_) => {
            sql_batch_result.set_ok(());
        },
        Err(err) => {
            let mut response_error = sql_batch_result.init_error().init_error();
            match err {
                SqlBatchError::DatabaseBusy => response_error.set_database_busy(()),
                SqlBatchError::BindingNotFound => response_error.set_binding_not_found(()),
                SqlBatchError::StatementFailed { reason } => response_error.set_statement_failed(&reason),
                SqlBatchError::RuntimeShutdown => response_error.set_runtime_shutdown(()),
                SqlBatchError::UnknownError => response_error.set_unknown_error(()),
                SqlBatchError::RuntimeError => response_error.set_runtime_error(()),
            }
        }
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);

    write_result(&mut caller, result_addr, SqlBatchResultSerializeResult {
        bytes_resource_id: bytes_resource_id.into(),
        bytes_length: bytes_length as u64,
    });

    0
}

pub(super) fn fx_migration_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let resource = caller.data_mut().resource_set.sql_migration_results.remove(resource_id.into()).unwrap();

    let mut message = capnp::message::Builder::new_default();

    let sql_migrate_result = message.init_root::<abi_sql_capnp::sql_migrate_result::Builder>();
    let mut sql_migrate_result = sql_migrate_result.init_result();

    match resource {
        Ok(_) => {
            sql_migrate_result.set_ok(());
        },
        Err(err) => {
            let mut response_error = sql_migrate_result.init_error().init_error();
            match err {
                SqlMigrationError::DatabaseBusy => response_error.set_database_busy(()),
                SqlMigrationError::BindingNotFound => response_error.set_binding_not_found(()),
                SqlMigrationError::MigrationExecutionError { message } => {
                    let mut execution_error = response_error.init_execution_error();
                    if let Some(message) = message {
                        execution_error.set_message(message);
                    }
                },
                SqlMigrationError::SqlError { message } => response_error.set_sql_error(message),
                SqlMigrationError::RuntimeShutdown => response_error.set_runtime_shutdown(()),
                SqlMigrationError::UnknownError => response_error.set_unknown_error(()),
                SqlMigrationError::RuntimeError => response_error.set_runtime_error(()),
            }
        }
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);

    write_result(&mut caller, result_addr, SqlMigrationResultSerializeResult {
        bytes_resource_id: bytes_resource_id.into(),
        bytes_length: bytes_length as u64,
    });

    0
}

pub(super) fn fx_fetch_result_future_poll(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let result = resource_poll(
        &mut caller,
        |s| &mut s.fetch_result_futures,
        |s| &mut s.fetch_results,
        resource_id
    );

    let result = FetchResultFuturePollResult {
        tag: match &result { Poll::Pending => 1, Poll::Ready(_) => 0 },
        _pad: Default::default(),
        fetch_result_resource_id: match result { Poll::Ready(v) => v.into(), Poll::Pending => 0 },
    };
    let result = result.as_bytes();

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);
    view.copy_from_slice(result_addr, result.len() as u64, result).unwrap();

    0
}

pub(super) fn fx_fetch_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let fetch_result = caller.data_mut().resource_set.fetch_results.remove(resource_id.into()).unwrap();

    let resource = match fetch_result {
        Ok(response) => {
            let (parts, body) = response.into_parts();
            let body = caller.data_mut().resource_set.http_bodies.insert(body);
            Ok(FetchResultWithBodyResource::new(parts, body))
        },
        Err(err) => Err(err),
    };

    let mut message = capnp::message::Builder::new_default();
    let response = message.init_root::<abi_http_capnp::fetch_result::Builder>();
    let response = response.init_result();

    match resource {
        Ok(ok) => {
            let mut ok_builder = response.init_ok();
            ok_builder.set_status(ok.parts.status.as_u16());
            let mut headers = ok_builder.reborrow().init_headers(ok.parts.headers.len() as u32);
            for (index, (name, value)) in ok.parts.headers.iter().enumerate() {
                let mut header = headers.reborrow().get(index as u32);
                header.set_name(name.as_str());
                header.set_value(value.to_str().unwrap());
            }
            ok_builder.reborrow().set_body_resource_id(ok.body.into());
        }
        Err(err) => {
            let mut error_builder = response.init_error().init_error();
            match err {
                FetchResultError::FailedToReadRequest => error_builder.set_failed_to_read_request(()),
                FetchResultError::BadRequest => error_builder.set_bad_request(()),
                FetchResultError::BodyHostResourceIdNotFound => error_builder.set_body_host_resource_id_not_found(()),
                FetchResultError::ConnectionFailed => error_builder.set_connection_failed(()),
                FetchResultError::ConnectionTimeout => error_builder.set_connection_timeout(()),
                FetchResultError::ResponseTimeout => error_builder.set_response_timeout(()),
                FetchResultError::FunctionNotFound => error_builder.set_function_not_found(()),
                FetchResultError::FunctionPanicked => error_builder.set_function_panicked(()),
                FetchResultError::FunctionCrashed => error_builder.set_function_crashed(()),
                FetchResultError::FunctionBusy => error_builder.set_function_busy(()),
                FetchResultError::FunctionIncorrectResponse => error_builder.set_function_incorrect_response(()),
                FetchResultError::RuntimeShutdown => error_builder.set_runtime_shutdown(()),
                FetchResultError::FunctionInstantiationError => error_builder.set_function_instantiation_error(()),
                FetchResultError::InternalRuntimeAssertionError
                | FetchResultError::InternalRuntimeTimeoutError => error_builder.set_runtime_internal_error(()),
            }
        }
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);
    let result = FetchResultSerializeResult {
        bytes_resource_id: bytes_resource_id.into(),
        bytes_length: bytes_length as u64,
    };
    let result = result.as_bytes();

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);
    view.copy_from_slice(result_addr, result.len() as u64, result).unwrap();

    0
}

pub(super) fn fx_http_body_poll_frame(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let waker = caller.data_mut().waker.clone().unwrap();
    let mut cx = std::task::Context::from_waker(&waker);

    let http_body = caller.data_mut().resource_set.http_bodies.get_mut(resource_id.into()).unwrap();

    let result = match http_body.0 {
        HttpBodyInner::Stream(ref mut stream) => stream.poll_next_unpin(&mut cx),
        HttpBodyInner::FunctionStream(ref mut v) =>  v.poll_next_unpin(&mut cx).map(|v| v.map(|v| Ok(hyper::body::Bytes::from(v.unwrap())))),
    };

    let result = result.map(|v| caller.data_mut().resource_set.http_frames.insert(v));

    write_result(
        &mut caller,
        result_addr,
        HttpBodyPollFrameResult {
            tag: match &result { Poll::Pending => 1, Poll::Ready(_) => 0 },
            _pad: Default::default(),
            http_frame_resource_id: match result { Poll::Ready(v) => v.into(), Poll::Pending => 0 },
        },
    );

    0
}

pub(super) fn fx_http_frame_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let http_frame = match caller.data_mut().resource_set.http_frames.remove(resource_id.into()) {
        Some(v) => v,
        None => return HttpFrameSerializeResultCode::NotFound as u64,
    };

    let mut message = capnp::message::Builder::new_default();
    let serialized_frame = message.init_root::<abi_http_capnp::http_body_frame::Builder>();
    let mut serialized_frame = serialized_frame.init_frame();

    match http_frame {
        Some(v) => serialized_frame.set_bytes(&v.unwrap().to_vec()),
        None => serialized_frame.set_stream_end(()),
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);

    write_result(
        &mut caller,
        result_addr,
        HttpFrameSerializeResult {
            bytes_resource_id: bytes_resource_id.into(),
            bytes_length: bytes_length as u64,
        }
    );

    0
}

pub(super) fn fx_blob_put_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let blob_put_result = match caller.data_mut().resource_set.blob_put_results.remove(resource_id.into()) {
        Some(v) => v,
        None => return BlobPutResultSerializeResultCode::NotFound as u64,
    };

    let mut message = capnp::message::Builder::new_default();
    let blob_put_response = message.init_root::<abi_blob_capnp::blob_put_result::Builder>();
    let mut response = blob_put_response.init_result();

    match blob_put_result {
        Ok(()) => response.set_ok(()),
        Err(BlobPutError::StorageError) => response.set_storage_error(()),
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);

    write_result(
        &mut caller,
        result_addr,
        BlobPutResultSerializeResult {
            bytes_resource_id: bytes_resource_id.into(),
            bytes_length: bytes_length as u64,
        },
    );

    BlobPutResultSerializeResultCode::Ok as u64
}

pub(super) fn fx_blob_get_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let blob_get_result = match caller.data_mut().resource_set.blob_get_responses.remove(resource_id.into()) {
        Some(v) => v,
        None => return BlobGetResultSerializeResultCode::NotFound as u64,
    };

    let mut message = capnp::message::Builder::new_default();
    let blob_get_response = message.init_root::<abi_blob_capnp::blob_get_response::Builder>();
    let mut response = blob_get_response.init_response();

    match blob_get_result {
        Ok(None) => response.set_not_found(()),
        Ok(Some(v)) => response.set_value(&v),
        Err(BlobGetError::BadRequestFailedToAccessMemory) => response.set_bad_request_failed_to_access_memory(()),
        Err(BlobGetError::BadRequestArgumentOutOfBounds) => response.set_bad_request_argument_out_of_bounds(()),
        Err(BlobGetError::BadRequestArgumentFailedToDecode) => response.set_bad_request_argument_failed_to_decode(()),
        Err(BlobGetError::BindingNotExists) => response.set_binding_not_exists(()),
        Err(BlobGetError::StorageError) => response.set_storage_error(()),
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);

    write_result(
        &mut caller,
        result_addr,
        BlobGetResultSerializeResult {
            bytes_resource_id: bytes_resource_id.into(),
            bytes_length: bytes_length as u64,
        },
    );

    BlobGetResultSerializeResultCode::Ok as u64
}

pub(super) fn fx_blob_delete_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let blob_delete_result = match caller.data_mut().resource_set.blob_delete_results.remove(resource_id.into()) {
        Some(v) => v,
        None => return BlobDeleteResultSerializeResultCode::NotFound as u64,
    };

    let mut message = capnp::message::Builder::new_default();
    let blob_delete_response = message.init_root::<abi_blob_capnp::blob_delete_response::Builder>();
    let mut response = blob_delete_response.init_response();

    match blob_delete_result {
        Ok(()) => response.set_ok(()),
        Err(BlobDeleteError::StorageError) => response.set_storage_error(()),
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);

    write_result(
        &mut caller,
        result_addr,
        BlobDeleteResultSerializeResult {
            bytes_resource_id: bytes_resource_id.into(),
            bytes_length: bytes_length as u64,
        }
    );

    BlobDeleteResultSerializeResultCode::Ok as u64
}

fn resource_poll<T: Clone, T2: From<slotmap::DefaultKey>, F, V>(
    caller: &mut wasmtime::Caller<'_, FunctionInstanceState>,
    resource_table_getter: impl FnOnce(&mut FunctionResources) -> &mut ResourceTable<T, F>,
    result_resource_table_getter: impl FnOnce(&mut FunctionResources) -> &mut ResourceTable<T2, V>,
    resource_id: impl Into<T>
) -> Poll<T2> where slotmap::DefaultKey: From<T>, F: Future<Output = V> + Unpin {
    let function_state = caller.data_mut();

    let waker = function_state.waker.clone().unwrap();
    let mut cx = std::task::Context::from_waker(&waker);

    let resource_table = resource_table_getter(&mut function_state.resource_set);

    let resource_id = resource_id.into();
    let future = resource_table.get_mut(resource_id.clone()).unwrap();
    match future.poll_unpin(&mut cx) {
        Poll::Pending => Poll::Pending,
        Poll::Ready(result) => {
            let _ = resource_table.remove(resource_id).unwrap();
            Poll::Ready(result_resource_table_getter(&mut function_state.resource_set).insert(result))
        }
    }
}

fn write_result(
    caller: &mut wasmtime::Caller<'_, FunctionInstanceState>,
    result_addr: u64,
    result: impl zerocopy::IntoBytes + zerocopy::Immutable,
) {
    let result = result.as_bytes();

    let memory = function_memory::FunctionMemory::from_caller(caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);

    view.copy_from_slice(result_addr, result.len() as u64, result).unwrap();
}

// TODO: refactor below
pub(super) fn fx_sql_exec_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: u64, req_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = {
        let ptr = req_addr as usize;
        let len = req_len as usize;
        &view[ptr..ptr+len]
    };
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<abi_sql_capnp::sql_exec_request::Reader>().unwrap();

    let binding = message.get_binding().unwrap().to_str().unwrap();
    let binding = match caller.data().bindings.sql.get(binding) {
        Some(v) => v,
        None => {
            return caller.data_mut().resource_set.sql_query_result_futures.insert(std::future::ready(Err(SqlQueryError::BindingNotFound)).boxed()).into();
        }
    };

    let (response_tx, response_rx) = oneshot::channel();
    caller.data().runtime_services.sql.send_message(SqlMessage::Exec(SqlExecMessage {
        binding: binding.clone(),
        statement: message.get_statement().unwrap().to_string().unwrap(),
        params: message.get_params().unwrap().into_iter()
            .map(|v| match v.get_value().which().unwrap() {
                abi_sql_capnp::sql_value::value::Null(_) => SqlValue::Null,
                abi_sql_capnp::sql_value::value::Integer(v) => SqlValue::Integer(v),
                abi_sql_capnp::sql_value::value::Real(v) => SqlValue::Real(v),
                abi_sql_capnp::sql_value::value::Which::Text(v) => SqlValue::Text(v.unwrap().to_string().unwrap()),
                abi_sql_capnp::sql_value::value::Which::Blob(v) => SqlValue::Blob(v.unwrap().to_vec()),
            })
            .collect(),
        response: response_tx,
    })).unwrap();

    caller.data_mut().resource_set.sql_query_result_futures.insert(async move {
        match response_rx.await {
            Ok(v) => v.map_err(|v| v.into()),
            Err(_) => Err(SqlQueryError::RuntimeShutdown),
        }
    }.boxed()).into()
}

pub(super) fn fx_sql_migrate_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: u64, req_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = {
        let ptr = req_addr as usize;
        let len = req_len as usize;

        &view[ptr..ptr+len]
    };
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<abi_sql_capnp::sql_migrate_request::Reader>().unwrap();

    let binding = message.get_binding().unwrap().to_str().unwrap();
    let binding = match caller.data().bindings.sql.get(binding) {
        Some(v) => v,
        None => return caller.data_mut().resource_set.sql_migration_result_futures.insert(std::future::ready(Err(SqlMigrationError::BindingNotFound)).boxed()).into(),
    };

    let (response_tx, response_rx) = oneshot::channel();
    let send_result = caller.data().runtime_services.sql.send_message_migrate(SqlMigrateMessage {
        binding: binding.clone(),
        migrations: message.get_migrations().unwrap().into_iter()
            .map(|v| v.unwrap().to_string().unwrap())
            .collect(),
        response: response_tx,
    });

    caller.data_mut().resource_set.sql_migration_result_futures.insert(async move {
        match send_result {
            Ok(_) => match response_rx.await {
                Ok(v) => v.map_err(SqlMigrationError::from),
                Err(_) => Err(SqlMigrationError::RuntimeShutdown),
            },
            Err(_) => Err(SqlMigrationError::RuntimeShutdown),
        }
    }.boxed()).into()
}

pub(super) fn fx_sql_batch_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: u64, req_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = {
        let ptr = req_addr as usize;
        let len = req_len as usize;
        &view[ptr..ptr+len]
    };
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<abi_sql_capnp::sql_batch_request::Reader>().unwrap();

    let binding = message.get_binding().unwrap().to_str().unwrap();
    let binding = match caller.data().bindings.sql.get(binding) {
        Some(v) => v,
        None => return caller.data_mut().resource_set.sql_batch_result_futures.insert(std::future::ready(Err(SqlBatchError::BindingNotFound)).boxed()).into(),
    };

    let queries: Vec<(String, Vec<SqlValue>)> = message.get_queries().unwrap().into_iter()
        .map(|query| {
            let statement = query.get_statement().unwrap().to_string().unwrap();
            let params = query.get_params().unwrap().into_iter()
                .map(|v| match v.get_value().which().unwrap() {
                    abi_sql_capnp::sql_value::value::Null(_) => SqlValue::Null,
                    abi_sql_capnp::sql_value::value::Integer(v) => SqlValue::Integer(v),
                    abi_sql_capnp::sql_value::value::Real(v) => SqlValue::Real(v),
                    abi_sql_capnp::sql_value::value::Which::Text(v) => SqlValue::Text(v.unwrap().to_string().unwrap()),
                    abi_sql_capnp::sql_value::value::Which::Blob(v) => SqlValue::Blob(v.unwrap().to_vec()),
                })
                .collect();
            (statement, params)
        })
        .collect();

    let (response_tx, response_rx) = oneshot::channel();
    caller.data().runtime_services.sql.send_message(SqlMessage::Batch(SqlBatchMessage {
        binding: binding.clone(),
        queries,
        response: response_tx,
    })).unwrap();

    caller.data_mut().resource_set.sql_batch_result_futures.insert(async move {
        match response_rx.await {
            Ok(v) => v.map_err(SqlBatchError::from),
            Err(_) => Err(SqlBatchError::RuntimeShutdown),
        }
    }.boxed()).into()
}

pub(super) fn fx_sleep_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, sleep_millis: u64) -> u64 {
    caller.data_mut().resource_set.unit_futures.insert(async move {
        tokio::time::sleep(Duration::from_millis(sleep_millis)).await;
    }.boxed()).into()
}

pub(super) fn fx_random_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, ptr: u64, len: u64) {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;
    let len = len as usize;

    rand::rngs::OsRng.try_fill_bytes(&mut view[ptr..ptr+len]).unwrap();
}

pub(super) fn fx_time_handler(_caller: wasmtime::Caller<'_, FunctionInstanceState>) -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

pub(super) fn fx_blob_put_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    binding_ptr: u64,
    binding_len: u64,
    key_ptr: u64,
    key_len: u64,
    value_ptr: u64,
    value_len: u64
) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_ptr as usize;
        let len = binding_len as usize;
        str::from_utf8(&view[ptr..ptr+len]).unwrap()
    };
    let bucket = caller.data().bindings.blob.get(binding).unwrap().bucket.clone();

    let key = {
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let value = {
        let ptr = value_ptr as usize;
        let len = value_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let blob_tx = caller.data().runtime_services.blob.clone();

    caller.data_mut().resource_set.blob_put_result_futures.insert(async move {
        let (result, result_rx) = oneshot::channel();

        blob_tx.send_async(BlobMessage::Put {
            bucket,
            key,
            value,
            result,
        }).await.unwrap();

        result_rx.await.unwrap().map_err(BlobPutError::from)
    }.boxed()).into()
}

pub(super) fn fx_blob_get_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    binding_ptr: u64,
    binding_len: u64,
    key_ptr: u64,
    key_len: u64,
) -> u64 {
    fn handle_ready_resource(caller: &mut wasmtime::Caller<'_, FunctionInstanceState>, resource: Result<Option<Vec<u8>>, BlobGetError>) -> BlobGetResponseFutureResourceKey {
        caller.data_mut().resource_set.blob_get_response_futures.insert(std::future::ready(resource).boxed())
    }

    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(err) => return handle_ready_resource(&mut caller, err.into()).into(),
    };
    let context = caller.as_context();
    let memory = memory.view(&context);

    let binding = match memory.str_ref(binding_ptr, binding_len) {
        Ok(v) => v,
        Err(err) => return handle_ready_resource(&mut caller, err.into()).into(),
    };
    let bucket = caller.data().bindings.blob.get(binding).map(|v| v.bucket.clone());

    let key = match memory.vec_clone(key_ptr, key_len) {
        Ok(v) => v,
        Err(err) => return handle_ready_resource(&mut caller, err.into()).into(),
    };

    let blob_tx = caller.data().runtime_services.blob.clone();

    caller.data_mut().resource_set.blob_get_response_futures.insert(async move {
        match bucket {
            Some(bucket) => {
                let (result, result_rx) = oneshot::channel();

                blob_tx.send(BlobMessage::Get {
                    bucket,
                    key,
                    result
                }).unwrap();

                match result_rx.await.unwrap() {
                    Ok(v) => Ok(v),
                    Err(crate::tasks::blob::GetError::BlobStorageError) => Err(BlobGetError::StorageError),
                }
            },
            None => Err(BlobGetError::BindingNotExists),
        }
    }.boxed()).into()
}

pub(super) fn fx_blob_delete_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    binding_ptr: u64,
    binding_len: u64,
    key_ptr: u64,
    key_len: u64,
) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_ptr as usize;
        let len = binding_len as usize;
        str::from_utf8(&view[ptr..ptr+len]).unwrap()
    };
    let bucket = caller.data().bindings.blob.get(binding).unwrap().bucket.clone();

    let key = {
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let blob_tx = caller.data().runtime_services.blob.clone();

    caller.data_mut().resource_set.blob_delete_result_futures.insert(async move {
        let (result, result_rx) = oneshot::channel();
        blob_tx.send_async(BlobMessage::Delete { bucket, key, result }).await.unwrap();
        result_rx.await.unwrap().map_err(BlobDeleteError::from)
    }.boxed()).into()
}

pub(super) fn fx_fetch_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    req_ptr: u64,
    req_len: u64,
) -> u64 {
    debug!("fx_fetch_handler - enter");

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).map_err(|_| FetchResultError::FailedToReadRequest);
    let context = caller.as_context();
    let view = memory.as_ref()
        .map_err(|err| (*err).clone())
        .map(|memory| memory.view(&context));

    let request = view.as_ref().map_err(|err| (*err).clone())
        .and_then(|view| view.slice(req_ptr, req_len).map_err(|_| FetchResultError::FailedToReadRequest));

    let request_reader = request
        .and_then(|mut request| capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default())
        .map_err(|_| FetchResultError::BadRequest));

    let request = match request_reader.as_ref() {
        Err(err) => Err(err.clone()),
        Ok(v) => v.get_root::<abi_http_capnp::http_request::Reader>().map_err(|_| FetchResultError::BadRequest),
    };

    let request_method = request.as_ref()
        .map_err(|err| err.clone())
        .and_then(|v| v.get_method().map_err(|_| FetchResultError::BadRequest))
        .map(|v| match v {
            abi_http_capnp::HttpMethod::Get => Method::GET,
            abi_http_capnp::HttpMethod::Put => Method::PUT,
            abi_http_capnp::HttpMethod::Post => Method::POST,
            abi_http_capnp::HttpMethod::Patch => Method::PATCH,
            abi_http_capnp::HttpMethod::Delete => Method::DELETE,
            abi_http_capnp::HttpMethod::Options => Method::OPTIONS,
            abi_http_capnp::HttpMethod::Head => Method::HEAD,
            abi_http_capnp::HttpMethod::Connect => Method::CONNECT,
            abi_http_capnp::HttpMethod::Trace => Method::TRACE,
        });

    let request_uri = request
        .as_ref().map_err(|err| err.clone())
        .and_then(|v| v.get_uri().map_err(|_| FetchResultError::BadRequest))
        .and_then(|v| v.to_str().map_err(|_| FetchResultError::BadRequest))
        .and_then(|v| v.parse().map_err(|_| FetchResultError::BadRequest));
    let request_host = request_uri.as_ref().map_err(|v| v.clone()).and_then(|v: &http::Uri| v.host().ok_or(FetchResultError::BadRequest).map(|v| v.to_owned().to_lowercase()));

    let mut outgoing_request = request_uri.and_then(|v| request_method.map(|t| (v, t))).map(|(request_uri, request_method)| {
        let mut outgoing_request = http::Request::new(());
        *outgoing_request.method_mut() = request_method;
        *outgoing_request.uri_mut() = request_uri;
        outgoing_request
    });

    let mut request_id = None;

    match request.as_ref().map_err(|err| err.clone()).and_then(|v| v.get_headers().map_err(|_| FetchResultError::BadRequest)) {
        Ok(headers) => {
            for header in headers.into_iter() {
                let name = header.get_name()
                    .map_err(|_| FetchResultError::BadRequest)
                    .and_then(|v| v.to_str().map_err(|_| FetchResultError::BadRequest));
                let name = match name {
                    Ok(v) => v,
                    Err(err) => {
                        outgoing_request = Err(err);
                        break;
                    }
                };

                let value = header.get_value()
                    .map_err(|_| FetchResultError::BadRequest)
                    .and_then(|v| v.to_str().map_err(|_| FetchResultError::BadRequest));
                let value = match value {
                    Ok(v) => v,
                    Err(err) => {
                        outgoing_request = Err(err);
                        break;
                    }
                };

                if name.eq_ignore_ascii_case("x-request-id") {
                    request_id = Some(value.to_owned());
                }

                let key = match http::HeaderName::from_str(name) {
                    Ok(v) => v,
                    Err(_) => {
                        outgoing_request = Err(FetchResultError::BadRequest);
                        break;
                    }
                };

                let value = match value.parse() {
                    Ok(v) => v,
                    Err(_) => {
                        outgoing_request = Err(FetchResultError::BadRequest);
                        break;
                    }
                };

                let outgoing_request = match outgoing_request.as_mut() {
                    Ok(v) => v,
                    Err(_) => break,
                };

                outgoing_request.headers_mut().append(key, value);
            }
        },
        Err(_) => {
            outgoing_request = Err(FetchResultError::BadRequest);
        }
    }

    let result = match request_host {
        Err(err) => std::future::ready(Err(err)).boxed_local(),
        Ok(request_host) => {
            if let Some(function_binding) = caller.data().bindings.functions.get(&request_host) {
                let response_rx = match outgoing_request {
                    Err(err) => Err(err),
                    Ok(outgoing_request) => Ok(caller.data().runtime_services.local_worker.invoke_function(function_binding.function_id.clone(), outgoing_request).boxed_local()),
                };

                async move { response_rx?.await.map_err(FetchResultError::from) }.boxed_local()
            } else if request_host == "kv.fx.internal" {
                let body = request
                    .and_then(|v| v.get_body().map_err(|_| FetchResultError::BadRequest))
                    .and_then(|v| v.get_body().which().map_err(|_| FetchResultError::BadRequest));

                let body = match body {
                    Err(err) => Err(err),
                    Ok(body) => match body {
                        abi_http_capnp::http_body::body::Which::Empty(_) => Ok(HttpBody::for_stream(futures::stream::empty().boxed())),
                        abi_http_capnp::http_body::body::Which::Bytes(v) => v.map_err(|_| FetchResultError::BadRequest).map(|v| HttpBody::for_bytes(v.to_vec().into())),
                        abi_http_capnp::http_body::body::Which::HostResource(v) =>
                            caller.data_mut().resource_set.http_bodies.remove(v.into())
                                .ok_or(FetchResultError::BodyHostResourceIdNotFound)
                                .map(|body| HttpBody::for_stream(BodyStream::new(body)
                                    .filter_map(|result| async {
                                        match result {
                                            Ok(frame) => frame.into_data().ok().map(Ok),
                                            Err(_) => Some(Err(HttpStreamError::FunctionRequestBodyStreamError)),
                                        }
                                    }).boxed())),
                        abi_http_capnp::http_body::body::Which::FunctionStream(resource_id) => caller.data_mut().self_instance.upgrade()
                            .ok_or(FetchResultError::InternalRuntimeAssertionError)
                            .map(|instance | HttpBody::for_function_stream(instance, resource_id.into())),
                    },
                };

                let request = body.and_then(|body| outgoing_request.map(|outgoing_request| http::Request::from_parts(outgoing_request.into_parts().0, body)));

                let response_future = match request {
                    Ok(request) => caller.data_mut().runtime_services.kv_service.call(request)
                        .map(|v| {
                            let (parts, body) = v.unwrap().into_parts();
                            let body = HttpBody::for_stream(TryStreamExt::map_err(body.into_data_stream(), |_| HttpStreamError::RpcResponseStreamError).boxed());
                            Ok(::http::Response::from_parts(parts, body))
                        })
                        .boxed_local(),
                    Err(err) => std::future::ready(Err(err)).boxed_local(),
                };

                tokio::task::spawn_local(response_future).map(|v| v.map_err(|_| FetchResultError::InternalRuntimeAssertionError).flatten()).boxed_local()
            } else {
                let body = {
                    let body_set_result = request
                        .as_ref()
                        .map_err(|err| err.clone())
                        .and_then(|v| v.get_body().map_err(|_| FetchResultError::BadRequest))
                        .map_err(|_| FetchResultError::BadRequest)
                        .and_then(|v| v.get_body().which().map_err(|_| FetchResultError::BadRequest));

                    match body_set_result {
                        Err(err) => Err(err),
                        Ok(v) => match v {
                            abi_http_capnp::http_body::body::Which::Empty(_) => Ok(reqwest::Body::default()),
                            abi_http_capnp::http_body::body::Which::Bytes(v) =>
                                v
                                    .map(|v| {
                                        reqwest::Body::from(v.to_vec())
                                    })
                                    .map_err(|_| FetchResultError::BadRequest),
                            abi_http_capnp::http_body::body::Which::HostResource(v) =>
                                caller.data_mut().resource_set.http_bodies.remove(v.into())
                                    .ok_or(FetchResultError::BodyHostResourceIdNotFound)
                                    .map(|body| {
                                        let stream = BodyStream::new(body)
                                            .filter_map(|result| async {
                                                match result {
                                                    Ok(frame) => frame.into_data().ok().map(Ok),
                                                    Err(e) => Some(Err(e)),
                                                }
                                            });
                                        reqwest::Body::wrap_stream(stream)
                                    }),
                            abi_http_capnp::http_body::body::Which::FunctionStream(resource_id) =>
                                caller.data_mut().self_instance.upgrade()
                                    .ok_or(FetchResultError::InternalRuntimeAssertionError)
                                    .map(|function_instance| {
                                        let reader = FunctionStreamReader::new(function_instance, resource_id.into());
                                        reqwest::Body::wrap_stream(send_wrapper::SendWrapper::new(reader))
                                    })
                        }
                    }
                };

                let request = body.and_then(|body| outgoing_request.map(|outgoing_request| http::Request::from_parts(outgoing_request.into_parts().0, body)));

                let fetch_request = request
                    .and_then(|request| reqwest::Request::try_from(request).map_err(|_| FetchResultError::InternalRuntimeAssertionError))
                    .map(|mut request| {
                        *request.timeout_mut() = Some(Duration::from_secs(3));
                        request
                    });

                let client = caller.data().http_client.clone();
                async move {
                    match client.execute(fetch_request?).await {
                        Ok(result) => {
                            let http_response: ::http::Response<reqwest::Body> = result.into();
                            let (parts, body) = http_response.into_parts();
                            let body = HttpBody::for_stream(body.into_data_stream().map_err(HttpStreamError::FetchResponseStreamError).boxed());
                            Ok(::http::Response::from_parts(parts, body))
                        }
                        Err(err) => {
                            warn!("fetch: external http request timeout: {err:?}, request_id: {:?}", request_id);
                            let error = if err.is_timeout() && err.is_connect() {
                                FetchResultError::ConnectionTimeout
                            } else if err.is_timeout() {
                                FetchResultError::ResponseTimeout
                            } else {
                                FetchResultError::ConnectionFailed
                            };
                            Err(error)
                        }
                    }
                }.boxed_local()
            }
        }
    };
    let result = caller.data_mut().resource_set.fetch_result_futures.insert(SendWrapper::new(result));

    debug!("fx_fetch_handler - exit");

    result.into()
}

pub(super) fn fx_metrics_counter_register_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_ptr: u64, req_len: u64, result_addr: u64) -> u64 {
    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(_) => return MetricsCounterRegisterResultCode::FailedToReadRequest as u64,
    };

    let context = caller.as_context();
    let view = memory.view(&context);

    let mut request = match view.slice(req_ptr, req_len) {
        Ok(v) => v,
        Err(_) => return MetricsCounterRegisterResultCode::FailedToReadRequest as u64,
    };

    let request_reader = match capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()) {
        Ok(v) => v,
        Err(_) => return MetricsCounterRegisterResultCode::BadRequest as u64,
    };
    let request = match request_reader.get_root::<abi_metrics_capnp::counter_register::Reader>() {
        Ok(v) => v,
        Err(_) => return MetricsCounterRegisterResultCode::BadRequest as u64,
    };

    let metric_key = MetricKey {
        name: match request.get_name().ok().and_then(|v| v.to_string().ok()) {
            Some(v) => v,
            None => return MetricsCounterRegisterResultCode::BadRequest as u64,
        },
        labels: {
            let request_labels = match request.get_labels() {
                Ok(v) => v,
                Err(_) => return MetricsCounterRegisterResultCode::BadRequest as u64,
            };

            let mut labels = Vec::with_capacity(request_labels.len() as usize);
            for label in request_labels.into_iter() {
                let name = match label.get_name().ok().and_then(|v| v.to_string().ok()) {
                    Some(v) => v,
                    None => return MetricsCounterRegisterResultCode::BadRequest as u64,
                };
                let value = match label.get_value().ok().and_then(|v| v.to_string().ok()) {
                    Some(v) => v,
                    None => return MetricsCounterRegisterResultCode::BadRequest as u64,
                };

                labels.push((name, value));
            }

            labels.sort();

            labels
        },
    };

    let counter_id = caller.data_mut().metrics.counter_register(metric_key);

    write_result(&mut caller, result_addr, MetricsCounterRegisterResult {
        counter_id: counter_id.into_abi(),
    });

    MetricsCounterRegisterResultCode::Ok as u64
}

pub(super) fn fx_metrics_counter_increment_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, counter_id: u64, delta: u64) {
    caller.data_mut().metrics.counter_increment(MetricId::from_abi(counter_id), delta);
}

pub(super) fn fx_metrics_gauge_update(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, gauge_id: u64, value: i64) {
    caller.data_mut().metrics.gauge_update(MetricId::from_abi(gauge_id), value);
}

pub(crate) fn fx_env_len_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, key_addr: u64, key_len: u64, result_addr: u64) -> u64 {
    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(_) => return EnvLenResultCode::FailedToReadRequest as u64,
    };

    let context = caller.as_context();
    let view = memory.view(&context);

    let key = match view.slice(key_addr, key_len) {
        Ok(v) => v,
        Err(_) => return EnvLenResultCode::FailedToReadRequest as u64,
    };
    let key = match str::from_utf8(key) {
        Ok(v) => v,
        Err(_) => return EnvLenResultCode::BadRequest as u64,
    };

    let len = match caller.data().bindings.env.get(key) {
        Some(v) => v.len() as u64,
        None => return EnvLenResultCode::NotFound as u64,
    };

    write_result(&mut caller, result_addr, EnvLenResult {
        len,
    });

    EnvLenResultCode::Ok as u64
}

pub(crate) fn fx_env_get_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, key_addr: u64, key_len: u64, value_addr: u64) -> u64 {
    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(_) => return EnvGetResult::FailedToReadRequest as u64,
    };
    let context = caller.as_context();
    let view = memory.view(&context);

    let key = match view.slice(key_addr, key_len) {
        Ok(v) => v,
        Err(_) => return EnvGetResult::FailedToReadRequest as u64,
    };
    let key = match str::from_utf8(key) {
        Ok(v) => v,
        Err(_) => return EnvGetResult::BadRequest as u64,
    };

    let value = match caller.data().bindings.env.get(key) {
        Some(value) => value.clone(),
        None => return EnvGetResult::NotFound as u64,
    };

    (match memory.view_mut(&mut caller.as_context_mut()).copy_from_slice(value_addr, value.len() as u64, value.as_bytes()) {
        Ok(_) => EnvGetResult::Ok,
        Err(_) => EnvGetResult::FailedToWriteValue,
    }) as u64
}

pub(crate) fn fx_kv_get_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, key_addr: u64, key_len: u64) -> u64 {
    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(_) => return caller.data_mut().resource_set.kv_get_response_futures.insert(ready(Err(KvGetHandlerError::FailedToReadRequest)).boxed()).into(),
    };

    let context = caller.as_context();
    let view = memory.view(&context);

    let binding = view.slice(binding_addr, binding_len).map_err(|_| KvGetHandlerError::FailedToReadRequest)
        .and_then(|binding| str::from_utf8(binding).map_err(|_| KvGetHandlerError::BadRequest));
    let namespace = binding
        .map(|binding| caller.data().bindings.kv.get(binding).map(|v| v.namespace.clone()));

    let key = view.vec_clone(key_addr, key_len).map_err(|_| KvGetHandlerError::FailedToReadRequest);
    let kv_tx = caller.data_mut().runtime_services.kv.clone();

    caller.data_mut().resource_set.kv_get_response_futures.insert(async move {
        let namespace = namespace?.ok_or(KvGetHandlerError::BindingNotFound)?;
        let key = key?;

        let (result_tx, result_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Get { key, result: result_tx },
        }).await.map_err(|_| KvGetHandlerError::RuntimeShutdown)?;

        match result_rx.await.map_err(|_| KvGetHandlerError::RuntimeShutdown)? {
            None => Err(KvGetHandlerError::KeyNotFound),
            Some(v) => Ok(v),
        }
    }.boxed()).into()
}

pub(crate) fn fx_kv_delex_ifeq_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, key_addr: u64, key_len: u64, ifeq_addr: u64, ifeq_len: u64) -> u64 {
    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(_) => return caller.data_mut().resource_set.kv_delex_result_futures.insert(ready(Err(KvDelexHandlerError::FailedToReadRequest)).boxed()).into(),
    };
    let context = caller.as_context();
    let view = memory.view(&context);

    let binding = {
        let binding = match view.slice(binding_addr, binding_len) {
            Ok(v) => v,
            Err(_) => return caller.data_mut().resource_set.kv_delex_result_futures.insert(ready(Err(KvDelexHandlerError::FailedToReadRequest)).boxed()).into(),
        };
        match str::from_utf8(binding) {
            Ok(v) => v,
            Err(_) => return caller.data_mut().resource_set.kv_delex_result_futures.insert(ready(Err(KvDelexHandlerError::BadRequest)).boxed()).into(),
        }
    };
    let namespace = caller.data().bindings.kv.get(binding).map(|v| v.namespace.clone());

    let key = match view.vec_clone(key_addr, key_len) {
        Ok(v) => v,
        Err(_) => return caller.data_mut().resource_set.kv_delex_result_futures.insert(ready(Err(KvDelexHandlerError::FailedToReadRequest)).boxed()).into(),
    };

    let ifeq = match view.vec_clone(ifeq_addr, ifeq_len) {
        Ok(v) => v,
        Err(_) => return caller.data_mut().resource_set.kv_delex_result_futures.insert(ready(Err(KvDelexHandlerError::FailedToReadRequest)).boxed()).into(),
    };

    let kv_tx = caller.data_mut().runtime_services.kv.clone();
    let (result_tx, result_rx) = oneshot::channel();

    caller.data_mut().resource_set.kv_delex_result_futures.insert(async move {
        let namespace = match namespace {
            Some(v) => v,
            None => return Err(KvDelexHandlerError::BindingNotFound),
        };

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Delex(KvDelexRequest { key, ifeq }, result_tx),
        }).await.map_err(|_| KvDelexHandlerError::RuntimeShutdown)?;

        result_rx.await.map_err(|_| KvDelexHandlerError::RuntimeShutdown)
    }.boxed()).into()
}

pub(crate) fn fx_kv_delex_result_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let kv_delex_result = caller.data_mut().resource_set.kv_delex_results.remove(resource_id.into());

    let mut message = capnp::message::Builder::new_default();
    let response = message.init_root::<abi_kv_capnp::kv_delex_result::Builder>();
    let mut response = response.init_result();

    match kv_delex_result {
        None => response.set_resource_not_found(()),
        Some(Ok(())) => response.set_ok(()),
        Some(Err(KvDelexHandlerError::BadRequest)) => response.set_bad_request(()),
        Some(Err(KvDelexHandlerError::FailedToReadRequest)) => response.set_failed_to_read_request(()),
        Some(Err(KvDelexHandlerError::RuntimeShutdown)) => response.set_runtime_shutdown(()),
        Some(Err(KvDelexHandlerError::BindingNotFound)) => response.set_binding_not_found(()),
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);

    write_result(&mut caller, result_addr, ResourceSerializeResult {
        bytes_resource_id: bytes_resource_id.into(),
        bytes_length: bytes_length as u64,
    });

    0
}

pub(crate) fn fx_kv_subscribe_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, channel_addr: u64, channel_len: u64) -> u64 {
    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(_) => return caller.data_mut().resource_set.kv_subscriptions.insert(Err(KvSubscriptionHandlerError::FailedToReadRequest)).into(),
    };
    let context = caller.as_context();
    let view = memory.view(&context);

    let binding = {
        let binding = match view.slice(binding_addr, binding_len) {
            Ok(v) => v,
            Err(_) => return caller.data_mut().resource_set.kv_subscriptions.insert(Err(KvSubscriptionHandlerError::FailedToReadRequest)).into(),
        };
        match str::from_utf8(binding) {
            Ok(v) => v,
            Err(_) => return caller.data_mut().resource_set.kv_subscriptions.insert(Err(KvSubscriptionHandlerError::BadRequest)).into(),
        }
    };
    let namespace = match caller.data().bindings.kv.get(binding) {
        Some(v) => v.namespace.clone(),
        None => return caller.data_mut().resource_set.kv_subscriptions.insert(Err(KvSubscriptionHandlerError::BindingNotFound)).into(),
    };

    let channel = match view.vec_clone(channel_addr, channel_len) {
        Ok(v) => v,
        Err(_)  => return caller.data_mut().resource_set.kv_subscriptions.insert(Err(KvSubscriptionHandlerError::FailedToReadRequest)).into(),
    };

    let (result_tx, result_rx) = oneshot::channel();
    let result = caller.data_mut().runtime_services.kv.send(KvMessage {
        namespace,
        operation: KvOperation::Subscribe { channel, result: result_tx },
    }).map(|()| KvSubscriptionResource::Init(result_rx)).map_err(|_| KvSubscriptionHandlerError::RuntimeShutdown);
    caller.data_mut().resource_set.kv_subscriptions.insert(result).into()
}

pub(crate) fn fx_kv_publish_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, channel_addr: u64, channel_len: u64, data_addr: u64, data_len: u64) -> u64 {
    let memory = match caller.get_export("memory").and_then(|v| v.into_memory()) {
        Some(v) => v,
        None => return caller.data_mut().resource_set.kv_publish_result_futures.insert(std::future::ready(Err(KvPublishHandlerError::FailedToReadRequest)).boxed()).into(),
    };
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_addr as usize;
        let len = binding_len as usize;
        let binding = &view[ptr..ptr+len];
        match str::from_utf8(binding) {
            Ok(v) => v,
            Err(_) => return caller.data_mut().resource_set.kv_publish_result_futures.insert(std::future::ready(Err(KvPublishHandlerError::BadRequest)).boxed()).into(),
        }
    };

    let namespace = match caller.data().bindings.kv.get(binding) {
        Some(v) => v.namespace.clone(),
        None => return caller.data_mut().resource_set.kv_publish_result_futures.insert(std::future::ready(Err(KvPublishHandlerError::BindingNotFound)).boxed()).into(),
    };

    let channel = {
        let ptr = channel_addr as usize;
        let len = channel_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let data = {
        let ptr = data_addr as usize;
        let len = data_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let (result_tx, result_rx) = oneshot::channel();
    let result = caller.data().runtime_services.kv.send(KvMessage {
        namespace,
        operation: KvOperation::Publish(KvPublishRequest {
            channel,
            data
        }, result_tx),
    }).map_err(|_| KvPublishHandlerError::RuntimeShutdown);

    caller.data_mut().resource_set.kv_publish_result_futures.insert(match result {
        Ok(()) => result_rx.map(|v| v.map_err(|_| KvPublishHandlerError::RuntimeShutdown)).boxed(),
        Err(err) => std::future::ready(Err(err)).boxed(),
    }).into()
}

pub(crate) fn fx_tasks_background_spawn_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, function_resource_id: u64) {
    let resource = FunctionResourceId::new(function_resource_id);
    caller.data_mut().tasks_background.push(resource);
}

macro_rules! future_poll_handler {
    ($name:ident, $futures_table:ident, $results_table:ident) => {
        pub(super) fn $name(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
            let result = resource_poll(
                &mut caller,
                |s| &mut s.$futures_table,
                |s| &mut s.$results_table,
                resource_id,
            );

            write_result(&mut caller, result_addr, AsyncResourcePollResult::from(result));

            0
        }
    };
}

future_poll_handler!(fx_kv_publish_result_future_poll, kv_publish_result_futures, kv_publish_results);
future_poll_handler!(fx_kv_delex_result_future_poll, kv_delex_result_futures, kv_delex_results);
future_poll_handler!(fx_migration_result_future_poll, sql_migration_result_futures, sql_migration_results);
future_poll_handler!(fx_blob_put_result_poll, blob_put_result_futures, blob_put_results);
future_poll_handler!(fx_blob_get_result_poll, blob_get_response_futures, blob_get_responses);
future_poll_handler!(fx_blob_delete_result_poll, blob_delete_result_futures, blob_delete_results);
