pub(crate) use fx_types::{
    capnp,
    abi_log_capnp,
    abi_sql_capnp,
    abi_http_capnp,
    abi_metrics_capnp,
    abi_blob_capnp,
    abi_kv_capnp,
    abi::{FuturePollResult, KvGetResponseSerializeResult},
};

use {
    std::{task::Poll, time::{SystemTime, UNIX_EPOCH}, str::FromStr, collections::HashMap},
    tokio::{sync::oneshot, time::Duration},
    tracing::{debug, error, warn},
    http::Method,
    http_body_util::{BodyStream, BodyExt},
    wasmtime::{AsContext, AsContextMut},
    futures::{FutureExt, StreamExt, TryStreamExt},
    rand::TryRngCore,
    send_wrapper::SendWrapper,
    zerocopy::IntoBytes,
    fx_types::abi::{
        ResourceMoveFromHostResult,
        KvGetResponseFuturePollResult,
        KvSetResponseFuturePollResult,
        KvSetResponseSerializeResult,
        UnitFuturePollResult,
        SqlQueryResultFuturePollResult,
        SqlQueryResultSerializeResult,
        FetchResultFuturePollResult,
        FetchResultSerializeResult,
        HttpBodyPollFrameResult,
        HttpFrameSerializeResult,
    },
    crate::{
        function::instance::FunctionInstanceState,
        resources::{
            Resource,
            ResourceId,
            FunctionResourceId,
            serialize::{SerializableResource, DeserializableResource, SerializedFunctionResource},
            future::FutureResource,
            resource::{
                ResourceTable,
                FunctionResources,
                FetchRequestHeaderResourceKey,
                KvGetResponseFutureResourceKey,
                KvSetResponseFutureResourceKey,
                UnitFutureResourceKey,
                SqlQueryResultFutureResourceKey,
            },
        },
        effects::{
            logs::{LogMessageEvent, LogSource, LogEventType, LogEventLevel, EventFieldValue},
            sql::{SqlValue, SqlBatchError, SqlMigrationError, SqlQueryError},
            blob::BlobGetResponse,
            fetch::{FetchResult, FetchResultWithBodyResource, FetchResultError, HttpStreamError},
            metrics::{MetricKey, MetricId},
            kv::{KvSetRequest, KvGetResponse, KvDelexRequest, KvSubscriptionResource, KvPublishRequest, KvSetError},
        },
        tasks::{
            sql::{SqlMessage, SqlExecMessage, SqlBatchMessage, SqlMigrateMessage},
            kv::{KvMessage, KvOperation},
            blob::BlobMessage,
        },
        triggers::http::{FetchRequestHeader, FunctionResponseInner, HttpBody, HttpBodyInner},
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

    if caller.data().logger_tx.send(message).is_err() {
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
    match fetch_request_header.body_resource_id {
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

pub(super) fn fx_kv_get_response_future_poll_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let result = {
        let key: KvGetResponseFutureResourceKey = resource_id.into();
        let function_state = caller.data_mut();

        let mut cx = std::task::Context::from_waker(function_state.waker.as_ref().unwrap());
        let future = function_state.resource_set.kv_get_response_futures.get_mut(key.clone()).unwrap();
        match future.poll_unpin(&mut cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(response) => {
                let _ = function_state.resource_set.kv_get_response_futures.remove(key).unwrap();
                Poll::Ready(function_state.resource_set.kv_get_responses.insert(response))
            }
        }
    };

    let result = match result {
        Poll::Pending => KvGetResponseFuturePollResult {
            tag: 1,
            _pad: Default::default(),
            kv_get_response_resource_id: 0,
        },
        Poll::Ready(kv_get_response_resource_id) => KvGetResponseFuturePollResult {
            tag: 0,
            _pad: Default::default(),
            kv_get_response_resource_id: kv_get_response_resource_id.into(),
        },
    };
    let result = result.as_bytes();

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);
    view.copy_from_slice(result_addr, result.len() as u64, result).unwrap();

    0
}

pub(super) fn fx_kv_get_response_serialize_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let response = caller.data_mut().resource_set.kv_get_responses.remove(resource_id.into()).unwrap();

    let mut message = capnp::message::Builder::new_default();
    let message_response = message.init_root::<abi_kv_capnp::kv_get_response::Builder>();
    let mut message_response = message_response.init_response();

    match response {
        KvGetResponse::KeyNotFound => message_response.set_key_not_found(()),
        KvGetResponse::Ok(v) => message_response.set_value(&v),
    }

    let bytes = capnp::serialize::write_message_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);
    let result = KvGetResponseSerializeResult {
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

pub(super) fn fx_kv_set_response_future_poll(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let result = {
        let key: KvSetResponseFutureResourceKey = resource_id.into();
        let function_state = caller.data_mut();

        let mut cx = std::task::Context::from_waker(function_state.waker.as_ref().unwrap());
        let future = function_state.resource_set.kv_set_response_futures.get_mut(key.clone()).unwrap();
        match future.poll_unpin(&mut cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(response) => {
                let _ = function_state.resource_set.kv_set_response_futures.remove(key).unwrap();
                Poll::Ready(function_state.resource_set.kv_set_responses.insert(response))
            }
        }
    };

    let result = match result {
        Poll::Pending => KvSetResponseFuturePollResult {
            tag: 1,
            _pad: Default::default(),
            kv_set_response_resource_id: 0,
        },
        Poll::Ready(kv_set_response_resource_id) => KvSetResponseFuturePollResult {
            tag: 0,
            _pad: Default::default(),
            kv_set_response_resource_id: kv_set_response_resource_id.into(),
        },
    };
    let result = result.as_bytes();

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);
    view.copy_from_slice(result_addr, result.len() as u64, result).unwrap();

    0
}

pub(super) fn fx_kv_set_response_serialize(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let kv_set_response = caller.data_mut().resource_set.kv_set_responses.remove(resource_id.into()).unwrap();

    let mut message = capnp::message::Builder::new_default();
    let response = message.init_root::<abi_kv_capnp::kv_set_response::Builder>();
    let mut response = response.init_response();

    match kv_set_response {
        Ok(v) => response.set_ok(v),
        Err(KvSetError::AlreadyExists) => response.set_already_exists(()),
    }

    let bytes = capnp::serialize::write_message_segments_to_words(&message);
    let bytes_length = bytes.len();
    let bytes_resource_id = caller.data_mut().resource_set.bytes.insert(bytes);
    let result = KvSetResponseSerializeResult {
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

pub(super) fn fx_sql_query_result_future_poll(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, result_addr: u64) -> u64 {
    let result = resource_poll(
        &mut caller,
        |s| &mut s.sql_query_result_futures,
        |s| &mut s.sql_query_results,
        resource_id
    );

    let result = match result {
        Poll::Pending => SqlQueryResultFuturePollResult {
            tag: 1,
            _pad: Default::default(),
            sql_query_result_resource_id: 0,
        },
        Poll::Ready(sql_query_result_resource_id) =>  SqlQueryResultFuturePollResult {
            tag: 0,
            _pad: Default::default(),
            sql_query_result_resource_id: sql_query_result_resource_id.into(),
        },
    };
    let result = result.as_bytes();

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);
    view.copy_from_slice(result_addr, result.len() as u64, result).unwrap();

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
        FetchResult::Inline(inline) => {
            let (parts, body) = inline.into_parts();
            let body = caller.data_mut().resource_set.http_bodies.insert(body);
            Ok(FetchResultWithBodyResource::new(parts, body))
        },
        FetchResult::BodyResource(v) => v,
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
                FetchResultError::ConnectionFailed => error_builder.set_connection_failed(()),
                FetchResultError::ConnectionTimeout => error_builder.set_connection_timeout(()),
                FetchResultError::ResponseTimeout => error_builder.set_response_timeout(()),
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
        HttpBodyInner::StreamLocal(ref mut stream) => stream.poll_next_unpin(&mut cx),
        HttpBodyInner::FunctionStream(ref mut v) =>  v.poll_next_unpin(&mut cx).map(|v| v.map(|v| Ok(hyper::body::Bytes::from(v)))),
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
    let http_frame = caller.data_mut().resource_set.http_frames.remove(resource_id.into()).unwrap();

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

fn resource_poll<T: Clone, T2: From<slotmap::DefaultKey>, F: Unpin, V>(
    caller: &mut wasmtime::Caller<'_, FunctionInstanceState>,
    resource_table_getter: impl FnOnce(&mut FunctionResources) -> &mut ResourceTable<T, F>,
    result_resource_table_getter: impl FnOnce(&mut FunctionResources) -> &mut ResourceTable<T2, V>,
    resource_id: impl Into<T>
) -> Poll<T2> where slotmap::DefaultKey: From<T>, F: Future<Output = V> {
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
pub(super) fn fx_resource_serialize_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) -> u64 {
    caller.data_mut().resource_serialize(&ResourceId::from(resource_id)) as u64
}

pub(super) fn fx_resource_move_from_host_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) -> u64 {
    let resource = match caller.data_mut().resource_remove(&ResourceId::from(resource_id)) {
        Resource::SqlMigrationResult(req) => match req {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::SqlBatchResult(req) => match req {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::BlobGetResult(res) => match res {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::KvSubscription(_) => panic!("resource of this type cannot be moved"),
    };

    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(err) => match err {
            function_memory::FunctionMemoryError::MemoryNotFound
            | function_memory::FunctionMemoryError::MemoryNotMemory => return ResourceMoveFromHostResult::FailedToAccessMemory as u64,
        }
    };
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);

    (match view.copy_from_slice(ptr, resource.len() as u64, &resource) {
        Ok(_) => ResourceMoveFromHostResult::Ok,
        Err(err) => match err {
            function_memory::FunctionMemoryAccessError::OutOfBounds => ResourceMoveFromHostResult::ArgumentOutOfMemoryBounds,
        }
    }) as u64
}

pub(super) fn fx_resource_drop_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) {
    let _ = caller.data_mut().resource_remove(&ResourceId::from(resource_id));
}

pub(super) fn fx_stream_frame_read_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) {
    debug!("fx_stream_frame_read_handler - enter");

    let serialized_frame = caller.data_mut().stream_read_frame(&ResourceId::from(resource_id));

    let memory = function_memory::FunctionMemory::from_caller(&mut caller).unwrap();
    let mut context = caller.as_context_mut();
    let mut view = memory.view_mut(&mut context);

    view.copy_from_slice(ptr, serialized_frame.len() as u64, &serialized_frame).unwrap();

    debug!("fx_stream_frame_read_handler - exit");
}

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
    let binding = match caller.data().bindings_sql.get(binding) {
        Some(v) => v,
        None => {
            return caller.data_mut().resource_set.sql_query_result_futures.insert(std::future::ready(Err(SqlQueryError::BindingNotFound)).boxed()).into();
        }
    };

    let (response_tx, response_rx) = oneshot::channel();
    caller.data().sql_controller.send_message(SqlMessage::Exec(SqlExecMessage {
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
    let binding = match caller.data().bindings_sql.get(binding) {
        Some(v) => v,
        None => {
            return caller.data_mut().resource_add(Resource::SqlMigrationResult(FutureResource::Ready(SerializableResource::Raw(
                Err(SqlMigrationError::BindingNotFound)
            )))).as_u64();
        }
    };

    let (response_tx, response_rx) = oneshot::channel();
    let send_result = caller.data().sql_controller.send_message_migrate(SqlMigrateMessage {
        binding: binding.clone(),
        migrations: message.get_migrations().unwrap().into_iter()
            .map(|v| v.unwrap().to_string().unwrap())
            .collect(),
        response: response_tx,
    });

    caller.data_mut().resource_add(Resource::SqlMigrationResult(FutureResource::for_future(async move {
        SerializableResource::Raw(match send_result {
            Ok(_) => match response_rx.await {
                Ok(v) => v.map_err(SqlMigrationError::from),
                Err(_) => Err(SqlMigrationError::RuntimeShutdown),
            },
            Err(_) => Err(SqlMigrationError::RuntimeShutdown),
        })
    }))).as_u64()
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
    let binding = match caller.data().bindings_sql.get(binding) {
        Some(v) => v,
        None => {
            return caller.data_mut().resource_add(Resource::SqlBatchResult(FutureResource::Ready(SerializableResource::Raw(
                Err(SqlBatchError::BindingNotFound)
            )))).as_u64();
        }
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
    caller.data().sql_controller.send_message(SqlMessage::Batch(SqlBatchMessage {
        binding: binding.clone(),
        queries,
        response: response_tx,
    })).unwrap();

    caller.data_mut().resource_add(Resource::SqlBatchResult(FutureResource::for_future(async move {
        SerializableResource::Raw(match response_rx.await {
            Ok(v) => v.map_err(SqlBatchError::from),
            Err(_) => Err(SqlBatchError::RuntimeShutdown),
        })
    }))).as_u64()
}

pub(super) fn fx_future_poll_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, future_resource_id: u64) -> i64 {
    let resource_id = ResourceId::new(future_resource_id);
    (match caller.data_mut().resource_poll(&resource_id) {
        Some(Poll::Pending) => FuturePollResult::Pending,
        Some(Poll::Ready(_)) => FuturePollResult::Ready,
        None => FuturePollResult::NotFound,
    }) as i64
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
    let bucket = caller.data().bindings_blob.get(binding).unwrap().bucket.clone();

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

    let blob_tx = caller.data().blob_tx.clone();

    caller.data_mut().resource_set.unit_futures.insert(async move {
        let (result, result_rx) = oneshot::channel();

        blob_tx.send_async(BlobMessage::Put {
            bucket,
            key,
            value,
            result,
        }).await.unwrap();

        result_rx.await.unwrap()
    }.boxed()).into()
}

pub(super) fn fx_blob_get_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    binding_ptr: u64,
    binding_len: u64,
    key_ptr: u64,
    key_len: u64,
) -> u64 {
    fn handle_ready_resource(caller: &mut wasmtime::Caller<'_, FunctionInstanceState>, resource: BlobGetResponse) -> u64 {
        caller.data_mut().resource_add(Resource::BlobGetResult(FutureResource::Ready(SerializableResource::Raw(resource)))).as_u64()
    }

    let memory = match function_memory::FunctionMemory::from_caller(&mut caller) {
        Ok(v) => v,
        Err(err) => return handle_ready_resource(&mut caller, BlobGetResponse::from(err)),
    };
    let context = caller.as_context();
    let memory = memory.view(&context);

    let binding = match memory.str_ref(binding_ptr, binding_len) {
        Ok(v) => v,
        Err(err) => return handle_ready_resource(&mut caller, BlobGetResponse::from(err)),
    };
    let bucket = caller.data().bindings_blob.get(binding).map(|v| v.bucket.clone());

    let key = match memory.vec_clone(key_ptr, key_len) {
        Ok(v) => v,
        Err(err) => return handle_ready_resource(&mut caller, BlobGetResponse::from(err)),
    };

    let blob_tx = caller.data().blob_tx.clone();

    caller.data_mut().resource_add(Resource::BlobGetResult(FutureResource::for_future(async move {
        SerializableResource::Raw({
            match bucket {
                Some(bucket) => {
                    let (result, result_rx) = oneshot::channel();

                    blob_tx.send(BlobMessage::Get {
                        bucket,
                        key,
                        result
                    }).unwrap();

                    match result_rx.await.unwrap() {
                        Some(v) => BlobGetResponse::Ok(v),
                        None => BlobGetResponse::NotFound,
                    }
                },
                None => BlobGetResponse::BindingNotExists
            }
        })
    }))).as_u64()
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
    let bucket = caller.data().bindings_blob.get(binding).unwrap().bucket.clone();

    let key = {
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let blob_tx = caller.data().blob_tx.clone();

    caller.data_mut().resource_set.unit_futures.insert(async move {
        let (result, result_rx) = oneshot::channel();
        blob_tx.send_async(BlobMessage::Delete { bucket, key, result }).await.unwrap();
        result_rx.await.unwrap();
    }.boxed()).into()
}

pub(super) fn fx_fetch_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    req_ptr: u64,
    req_len: u64,
) -> u64 {
    debug!("fx_fetch_handler - enter");

    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut request = {
        let ptr = req_ptr as usize;
        let len = req_len as usize;
        &view[ptr..ptr+len]
    };

    let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
    let request = request_reader.get_root::<abi_http_capnp::http_request::Reader>().unwrap();

    let request_method = match request.get_method().unwrap() {
        abi_http_capnp::HttpMethod::Get => Method::GET,
        abi_http_capnp::HttpMethod::Put => Method::PUT,
        abi_http_capnp::HttpMethod::Post => Method::POST,
        abi_http_capnp::HttpMethod::Patch => Method::PATCH,
        abi_http_capnp::HttpMethod::Delete => Method::DELETE,
        abi_http_capnp::HttpMethod::Options => Method::OPTIONS,
        abi_http_capnp::HttpMethod::Head => Method::HEAD,
        abi_http_capnp::HttpMethod::Connect => Method::CONNECT,
        abi_http_capnp::HttpMethod::Trace => Method::TRACE,
    };
    let request_uri = reqwest::Url::parse(request.get_uri().unwrap().to_str().unwrap()).unwrap();
    let request_host = request_uri.host_str().unwrap().to_owned().to_lowercase();

    let result = if let Some(function_binding) = caller.data().bindings_functions.get(&request_host) {
        let mut http_builder = http::Request::builder()
            .method(request_method)
            .uri(http::Uri::from_str(request.get_uri().unwrap().to_str().unwrap()).unwrap());
        for header in request.get_headers().unwrap().into_iter() {
            let name = header.get_name().unwrap().to_str().unwrap();
            let value = header.get_value().unwrap().to_str().unwrap();
            http_builder = http_builder.header(name, value);
        }
        let header = FetchRequestHeader::from_http_parts(
            http_builder.body(()).unwrap().into_parts().0
        );
        let response_rx = caller.data().local_worker.invoke_function(function_binding.function_id.clone(), header);

        async move {
            let response = response_rx.await.unwrap();
            let response = response.move_to_host().await.unwrap();
            match response.0 {
                FunctionResponseInner::HttpResponse(response) => {
                    // todo: make body lazy, support streaming
                    let body = response.body.replace(None).unwrap();
                    let (instance, body) = body.consume();
                    let body = DeserializableResource::from_serialized(SerializedFunctionResource::<HttpBody>::new(instance, body));
                    let body = body.copy_to_host().await.unwrap();

                    let http_response = ::http::Response::builder()
                        .status(response.status)
                        .body(())
                        .unwrap();
                    let (mut parts, _) = http_response.into_parts();
                    parts.headers = response.headers;
                    FetchResult::new(parts, body)
                }
            }
        }.boxed_local()
    } else {
        let mut fetch_request = reqwest::Request::new(
            request_method,
            request_uri
        );

        *fetch_request.timeout_mut() = Some(Duration::from_secs(3));

        for header in request.get_headers().unwrap().into_iter() {
            let name = header.get_name().unwrap().to_str().unwrap();
            let value = header.get_value().unwrap().to_str().unwrap();
            fetch_request.headers_mut().insert(name.parse::<http::header::HeaderName>().unwrap(), value.parse::<http::header::HeaderValue>().unwrap());
        }

        match request.get_body().unwrap().get_body().which().unwrap() {
            abi_http_capnp::http_body::body::Which::Empty(_) => {},
            abi_http_capnp::http_body::body::Which::Bytes(v) => {
                *fetch_request.body_mut() = Some(reqwest::Body::from(v.unwrap().to_vec()));
            },
            abi_http_capnp::http_body::body::Which::HostResource(v) => {
                let body = caller.data_mut().resource_set.http_bodies.remove(v.into()).unwrap();
                let stream = BodyStream::new(body)
                    .filter_map(|result| async {
                        match result {
                            Ok(frame) => frame.into_data().ok().map(Ok),
                            Err(e) => Some(Err(e)),
                        }
                    });
                *fetch_request.body_mut() = Some(reqwest::Body::wrap_stream(stream));
            },
            abi_http_capnp::http_body::body::Which::FunctionStream(_) => todo!(),
        }

        let client = caller.data().http_client.clone();
        async move {
            match client.execute(fetch_request).await {
                Ok(result) => {
                    let http_response: ::http::Response<reqwest::Body> = result.into();
                    let (parts, body) = http_response.into_parts();
                    let body = HttpBody::for_stream(body.into_data_stream().map_err(|err| HttpStreamError::FetchResponseStreamError(err)).boxed());
                    FetchResult::new(parts, body)
                }
                Err(err) => {
                    let error = if err.is_timeout() && err.is_connect() {
                        FetchResultError::ConnectionTimeout
                    } else if err.is_timeout() {
                        FetchResultError::ResponseTimeout
                    } else {
                        FetchResultError::ConnectionFailed
                    };
                    FetchResult::error(error)
                }
            }
        }.boxed_local()
    };
    let result = caller.data_mut().resource_set.fetch_result_futures.insert(SendWrapper::new(result));

    debug!("fx_fetch_handler - exit");

    result.into()
}

pub(super) fn fx_metrics_counter_register_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_ptr: u64, req_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut request = {
        let ptr = req_ptr as usize;
        let len = req_len as usize;
        &view[ptr..ptr+len]
    };

    let request_reader = capnp::serialize::read_message_from_flat_slice(&mut request, capnp::message::ReaderOptions::default()).unwrap();
    let request = request_reader.get_root::<abi_metrics_capnp::counter_register::Reader>().unwrap();

    let metric_key = MetricKey {
        name: request.get_name().unwrap().to_string().unwrap(),
        labels: {
            let mut labels = request.get_labels().unwrap().into_iter()
                .map(|v| (
                    v.get_name().unwrap().to_string().unwrap(),
                    v.get_value().unwrap().to_string().unwrap()
                ))
                .collect::<Vec<_>>();

            labels.sort();

            labels
        },
    };

    caller.data_mut().metrics.counter_register(metric_key).into_abi()
}

pub(super) fn fx_metrics_counter_increment_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, counter_id: u64, delta: u64) {
    caller.data_mut().metrics.counter_increment(MetricId::from_abi(counter_id), delta);
}

pub(crate) fn fx_env_len_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, key_addr: u64, key_len: u64) -> i64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let key = {
        let ptr = key_addr as usize;
        let len = key_len as usize;
        str::from_utf8(&view[ptr..ptr+len]).unwrap()
    };

    match caller.data().env.get(key) {
        Some(value) => value.len() as i64,
        None => -1,
    }
}

pub(crate) fn fx_env_get_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, key_addr: u64, key_len: u64, value_addr: u64) {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let (view, state) = memory.data_and_store_mut(&mut context);

    let key = {
        let ptr = key_addr as usize;
        let len = key_len as usize;
        str::from_utf8(&view[ptr..ptr+len]).unwrap()
    };

    let value = match state.env.get(key) {
        Some(value) => value,
        None => return,
    };
    {
        let ptr = value_addr as usize;
        let len = value.len();
        view[ptr..ptr+len].copy_from_slice(value.as_bytes());
    }
}

pub(crate) fn fx_kv_set_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, key_addr: u64, key_len: u64, value_addr: u64, value_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_addr as usize;
        let len = binding_len as usize;
        let binding = &view[ptr..ptr+len];
        str::from_utf8(binding).unwrap()
    };
    let namespace = caller.data().bindings_kv.get(binding).unwrap().namespace.clone();

    let key = {
        let ptr = key_addr as usize;
        let len = key_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let value = {
        let ptr = value_addr as usize;
        let len = value_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let kv_tx = caller.data_mut().kv_tx.clone();

    let req = KvSetRequest::new(key, value);

    caller.data_mut().resource_set.kv_set_response_futures.insert(async move {
        let (on_done, on_done_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Set(req, on_done),
        }).await.unwrap();

        on_done_rx.await.unwrap().unwrap();

        Ok(())
    }.boxed()).into()
}

pub(crate) fn fx_kv_set_nx_px_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, key_addr: u64, key_len: u64, value_addr: u64, value_len: u64, nx: u32, px: i64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_addr as usize;
        let len = binding_len as usize;
        let binding = &view[ptr..ptr+len];
        str::from_utf8(binding).unwrap()
    };
    let namespace = caller.data().bindings_kv.get(binding).unwrap().namespace.clone();

    let key = {
        let ptr = key_addr as usize;
        let len = key_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let value = {
        let ptr = value_addr as usize;
        let len = value_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let kv_tx = caller.data_mut().kv_tx.clone();

    let req = KvSetRequest::new(key, value)
        .with_nx(nx != 0)
        .with_px(if px > 0  { Some(Duration::from_millis(px as u64)) } else { None });

    caller.data_mut().resource_set.kv_set_response_futures.insert(async move {
        let (on_done, on_done_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Set(req, on_done),
        }).await.unwrap();

        on_done_rx.await.unwrap()
    }.boxed()).into()
}

pub(crate) fn fx_kv_get_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, key_addr: u64, key_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_addr as usize;
        let len = binding_len as usize;
        let binding = &view[ptr..ptr+len];
        str::from_utf8(binding).unwrap()
    };
    let namespace = caller.data().bindings_kv.get(binding).unwrap().namespace.clone();

    let key = {
        let ptr = key_addr as usize;
        let len = key_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let kv_tx = caller.data_mut().kv_tx.clone();

    caller.data_mut().resource_set.kv_get_response_futures.insert(async move {
        let (result_tx, result_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Get { key, result: result_tx },
        }).await.unwrap();

        match result_rx.await.unwrap() {
            None => KvGetResponse::KeyNotFound,
            Some(v) => KvGetResponse::Ok(v),
        }
    }.boxed()).into()
}

pub(crate) fn fx_kv_delex_ifeq_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, key_addr: u64, key_len: u64, ifeq_addr: u64, ifeq_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_addr as usize;
        let len = binding_len as usize;
        let binding = &view[ptr..ptr+len];
        str::from_utf8(binding).unwrap()
    };
    let namespace = caller.data().bindings_kv.get(binding).unwrap().namespace.clone();

    let key = {
        let ptr = key_addr as usize;
        let len = key_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let ifeq = {
        let ptr = ifeq_addr as usize;
        let len = ifeq_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let kv_tx = caller.data_mut().kv_tx.clone();
    let (result_tx, result_rx) = oneshot::channel();

    caller.data_mut().resource_set.unit_futures.insert(async move {
        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Delex(KvDelexRequest { key, ifeq }, result_tx),
        }).await.unwrap();

        result_rx.await.unwrap()
    }.boxed()).into()
}

pub(crate) fn fx_kv_subscribe_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, channel_addr: u64, channel_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_addr as usize;
        let len = binding_len as usize;
        let binding = &view[ptr..ptr+len];
        str::from_utf8(binding).unwrap()
    };
    let namespace = caller.data().bindings_kv.get(binding).unwrap().namespace.clone();

    let channel = {
        let ptr = channel_addr as usize;
        let len = channel_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    let (result_tx, result_rx) = oneshot::channel();
    caller.data_mut().kv_tx.send(KvMessage {
        namespace,
        operation: KvOperation::Subscribe { channel, result: result_tx },
    }).unwrap();

    caller.data_mut().resource_add(Resource::KvSubscription(KvSubscriptionResource::Init(result_rx))).as_u64()
}

pub(crate) fn fx_kv_publish_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, binding_addr: u64, binding_len: u64, channel_addr: u64, channel_len: u64, data_addr: u64, data_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let binding = {
        let ptr = binding_addr as usize;
        let len = binding_len as usize;
        let binding = &view[ptr..ptr+len];
        str::from_utf8(binding).unwrap()
    };
    let namespace = caller.data().bindings_kv.get(binding).unwrap().namespace.clone();

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
    caller.data().kv_tx.send(KvMessage {
        namespace,
        operation: KvOperation::Publish(KvPublishRequest {
            channel,
            data
        }, result_tx),
    }).unwrap();

    caller.data_mut().resource_set.unit_futures.insert(async move { result_rx.await.unwrap(); }.boxed()).into()
}

pub(crate) fn fx_tasks_background_spawn_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, function_resource_id: u64) {
    let resource = FunctionResourceId::new(function_resource_id);
    caller.data_mut().tasks_background.push(resource);
}
