pub(crate) use fx_types::{capnp, abi_log_capnp, abi_sql_capnp, abi_http_capnp, abi_metrics_capnp, abi_blob_capnp, abi::FuturePollResult};

use {
    std::{task::Poll, time::{SystemTime, UNIX_EPOCH}, str::FromStr},
    tokio::{sync::oneshot, time::Duration},
    http::Method,
    http_body_util::{BodyStream, BodyExt},
    wasmtime::{AsContext, AsContextMut},
    futures::{FutureExt, StreamExt, TryStreamExt},
    rand::TryRngCore,
    crate::{
        function::instance::FunctionInstanceState,
        resources::{
            Resource,
            ResourceId,
            serialize::{SerializableResource, DeserializableResource, SerializedFunctionResource},
            future::FutureResource,
            FunctionResourceId,
        },
        effects::{
            logs::{LogMessageEvent, LogSource, LogEventType, LogEventLevel, EventFieldValue},
            sql::{SqlValue, SqlBatchError, SqlMigrationError, SqlQueryError},
            blob::BlobGetResponse,
            fetch::{FetchResult, FetchResultWithBodyResource, HttpStreamError},
            metrics::{MetricKey, MetricId},
            kv::{KvSetRequest, KvGetResponse, KvDelexRequest, KvSubscriptionResource, KvPublishRequest},
        },
        tasks::{
            sql::{SqlMessage, SqlExecMessage, SqlBatchMessage, SqlMigrateMessage},
            kv::{KvMessage, KvOperation},
            blob::BlobMessage,
        },
        triggers::http::{FetchRequestBodyInner, FetchRequestHeader, FunctionResponseInner, HttpBody},
    },
};

pub(super) fn fx_log_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: i64, req_len: i64) {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = &view[req_addr as usize..(req_addr + req_len) as usize];
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<abi_log_capnp::log_message::Reader>().unwrap();

    let message: LogMessageEvent = LogMessageEvent::new(
        LogSource::function(&caller.data().function_id),
        match message.get_event_type().unwrap() {
            abi_log_capnp::EventType::Begin => LogEventType::Begin,
            abi_log_capnp::EventType::End => LogEventType::End,
            abi_log_capnp::EventType::Instant => LogEventType::Instant,
        },
        match message.get_level().unwrap() {
            abi_log_capnp::LogLevel::Trace => LogEventLevel::Trace,
            abi_log_capnp::LogLevel::Debug => LogEventLevel::Debug,
            abi_log_capnp::LogLevel::Info => LogEventLevel::Info,
            abi_log_capnp::LogLevel::Warn => LogEventLevel::Warn,
            abi_log_capnp::LogLevel::Error => LogEventLevel::Error,
        },
        message.get_fields().unwrap()
            .into_iter()
            .map(|v| (
                v.get_name().unwrap().to_string().unwrap(),
                EventFieldValue::Text(v.get_value().unwrap().to_string().unwrap())
            ))
            .collect()
    ).into();

    caller.data().logger_tx.send(message).unwrap();
}

pub(super) fn fx_resource_serialize_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) -> u64 {
    caller.data_mut().resource_serialize(&ResourceId::from(resource_id)) as u64
}

pub(super) fn fx_resource_move_from_host_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) {
    let resource = match caller.data_mut().resource_remove(&ResourceId::from(resource_id)) {
        Resource::FetchRequest(req) => req.into_serialized(),
        Resource::SqlQueryResult(req) => match req {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::SqlMigrationResult(req) => match req {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::SqlBatchResult(req) => match req {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::UnitFuture(_) => panic!("unit future cannot be moved to function"),
        Resource::BlobGetResult(res) => match res {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::FetchResult(res) => match res {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(resource) => match resource {
                FetchResult::Inline(resource) => {
                    let (parts, body) = resource.into_parts();
                    let body = caller.data_mut().resource_add(Resource::HttpBody(body));
                    SerializableResource::Raw(FetchResultWithBodyResource::new(parts, body)).into_serialized()
                },
                FetchResult::BodyResource(resource) => resource.into_serialized(),
            },
        },
        Resource::RequestBody(_) => panic!("resource of this type cannot be moved"),
        Resource::HttpBody(_) => panic!("resource of this type cannot be moved"),
        Resource::KvGetResult(v) => match v {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::KvSetResult(v) => match v {
            FutureResource::Future(_) => panic!("cannot move resouce that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::KvSubscription(_) => panic!("resource of this type cannot be moved"),
    };

    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;

    view[ptr..ptr+resource.len()].copy_from_slice(&resource);
}

pub(super) fn fx_resource_drop_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) {
    let _ = caller.data_mut().resource_remove(&ResourceId::from(resource_id));
}

pub(super) fn fx_stream_frame_read_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) {
    let serialized_frame = caller.data_mut().stream_read_frame(&ResourceId::from(resource_id));

    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;

    view[ptr..ptr+serialized_frame.len()].copy_from_slice(&serialized_frame);
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
            return caller.data_mut().resource_add(Resource::SqlQueryResult(FutureResource::Ready(SerializableResource::Raw(
                Err(SqlQueryError::BindingNotFound)
            )))).as_u64();
        }
    };

    let (response_tx, response_rx) = oneshot::channel();
    caller.data().sql_tx.send(SqlMessage::Exec(SqlExecMessage {
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

    caller.data_mut().resource_add(Resource::SqlQueryResult(FutureResource::for_future(async move {
        SerializableResource::Raw(response_rx.await.unwrap().map_err(|v| v.into()))
    }))).as_u64()
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
    caller.data().sql_tx.send(SqlMessage::Migrate(SqlMigrateMessage {
        binding: binding.clone(),
        migrations: message.get_migrations().unwrap().into_iter()
            .map(|v| v.unwrap().to_string().unwrap())
            .collect(),
        response: response_tx,
    })).unwrap();

    caller.data_mut().resource_add(Resource::SqlMigrationResult(FutureResource::for_future(async move {
        SerializableResource::Raw(response_rx.await.unwrap())
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
    caller.data().sql_tx.send(SqlMessage::Batch(SqlBatchMessage {
        binding: binding.clone(),
        queries,
        response: response_tx,
    })).unwrap();

    caller.data_mut().resource_add(Resource::SqlBatchResult(FutureResource::for_future(async move {
        SerializableResource::Raw(response_rx.await.unwrap())
    }))).as_u64()
}

pub(super) fn fx_future_poll_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, future_resource_id: u64) -> i64 {
    let resource_id = ResourceId::new(future_resource_id);
    (match caller.data_mut().resource_poll(&resource_id) {
        Poll::Pending => FuturePollResult::Pending,
        Poll::Ready(_) => FuturePollResult::Ready,
    }) as i64
}

pub(super) fn fx_sleep_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, sleep_millis: u64) -> u64 {
    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        tokio::time::sleep(Duration::from_millis(sleep_millis)).await;
    }.boxed())).as_u64()
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

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        let (result, result_rx) = oneshot::channel();

        blob_tx.send_async(BlobMessage::Put {
            bucket,
            key,
            value,
            result,
        }).await.unwrap();

        result_rx.await.unwrap()
    }.boxed())).as_u64()
}

pub(super) fn fx_blob_get_handler(
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
    let bucket = caller.data().bindings_blob.get(binding).map(|v| v.bucket.clone());

    let key = {
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        view[ptr..ptr+len].to_vec()
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

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        let (result, result_rx) = oneshot::channel();
        blob_tx.send_async(BlobMessage::Delete { bucket, key, result }).await.unwrap();
        result_rx.await.unwrap();
    }.boxed())).as_u64()
}

pub(super) fn fx_fetch_handler(
    mut caller: wasmtime::Caller<'_, FunctionInstanceState>,
    req_ptr: u64,
    req_len: u64,
) -> u64 {
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

    if let Some(function_binding) = caller.data().bindings_functions.get(&request_host) {
        let header = FetchRequestHeader::from_http_parts(
            http::Request::builder()
                .method(request_method)
                .uri(http::Uri::from_str(request.get_uri().unwrap().to_str().unwrap()).unwrap())
                .body(())
                .unwrap()
                .into_parts()
                .0
        );
        let response_rx = caller.data().local_worker.invoke_function(function_binding.function_id.clone(), header);

        caller.data_mut().resource_add(Resource::FetchResult(FutureResource::for_future(async move {
            let response = response_rx.await.unwrap();
            let response = response.move_to_host().await;
            match response.0 {
                FunctionResponseInner::HttpResponse(response) => {
                    // todo: make body lazy, support streaming
                    let body = response.body.replace(None).unwrap();
                    let (instance, body) = body.consume();
                    let body = DeserializableResource::from_serialized(SerializedFunctionResource::<HttpBody>::new(instance, body));
                    let body = body.copy_to_host().await;

                    let http_response = ::http::Response::builder()
                        .status(response.status)
                        .body(())
                        .unwrap();
                    let (mut parts, _) = http_response.into_parts();
                    parts.headers = response.headers;
                    FetchResult::new(parts, body)
                }
            }
        }))).as_u64()
    } else {
        let mut fetch_request = reqwest::Request::new(
            request_method,
            request_uri
        );

        match request.get_body().unwrap().get_body().which().unwrap() {
            abi_http_capnp::http_body::body::Which::Empty(_) => {},
            abi_http_capnp::http_body::body::Which::Bytes(v) => {
                *fetch_request.body_mut() = Some(reqwest::Body::from(v.unwrap().to_vec()));
            },
            abi_http_capnp::http_body::body::Which::HostResource(v) => {
                let resource_id = ResourceId::new(v);
                match caller.data_mut().resource_remove(&resource_id) {
                    Resource::BlobGetResult(_)
                    | Resource::FetchRequest(_)
                    | Resource::FetchResult(_)
                    | Resource::SqlQueryResult(_)
                    | Resource::SqlBatchResult(_)
                    | Resource::SqlMigrationResult(_)
                    | Resource::UnitFuture(_)
                    | Resource::KvSetResult(_)
                    | Resource::KvGetResult(_)
                    | Resource::KvSubscription(_) => panic!("this resource cannot be used as request body"),
                    Resource::RequestBody(v) => match v.0 {
                        FetchRequestBodyInner::FullSerialized(_)
                        | FetchRequestBodyInner::PartiallyReadStream { .. }
                        | FetchRequestBodyInner::PartiallyReadStreamSerialized { .. } => panic!("this body type cannot be used as request body"),
                        FetchRequestBodyInner::Full(bytes) => {
                            *fetch_request.body_mut() = Some(reqwest::Body::from(bytes));
                        },
                        FetchRequestBodyInner::Stream(stream) => {
                            let body_stream = BodyStream::new(stream)
                                .filter_map(|result| async {
                                    match result {
                                        Ok(frame) => frame.into_data().ok().map(Ok),
                                        Err(e) => Some(Err(e)),
                                    }
                                });
                            *fetch_request.body_mut() = Some(reqwest::Body::wrap_stream(body_stream));
                        }
                    },
                    Resource::HttpBody(body) => {
                        let stream = BodyStream::new(body)
                            .filter_map(|result| async {
                                match result {
                                    Ok(frame) => frame.into_data().ok().map(Ok),
                                    Err(e) => Some(Err(e)),
                                }
                            });
                        *fetch_request.body_mut() = Some(reqwest::Body::wrap_stream(stream));
                    },
                }
            },
            abi_http_capnp::http_body::body::Which::FunctionStream(_) => todo!(),
        }

        let client = caller.data().http_client.clone();
        caller.data_mut().resource_add(Resource::FetchResult(FutureResource::for_future(async move {
            let result = client.execute(fetch_request).await.unwrap();
            let http_response: ::http::Response<reqwest::Body> = result.into();
            let (parts, body) = http_response.into_parts();
            let body = HttpBody::for_stream(body.into_data_stream().map_err(|err| HttpStreamError::FetchResponseStreamError(err)).boxed());
            FetchResult::new(parts, body)
        }))).as_u64()
    }
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

pub(crate) fn fx_env_len_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, key_addr: u64, key_len: u64) -> u64 {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let key = {
        let ptr = key_addr as usize;
        let len = key_len as usize;
        str::from_utf8(&view[ptr..ptr+len]).unwrap()
    };

    caller.data().env.get(key).unwrap().len() as u64
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

    let value = state.env.get(key).unwrap();
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

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        let (on_done, on_done_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Set(req, on_done),
        }).await.unwrap();

        on_done_rx.await.unwrap().unwrap();
    }.boxed())).as_u64()
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

    caller.data_mut().resource_add(Resource::KvSetResult(FutureResource::for_future(async move {
        let (on_done, on_done_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Set(req, on_done),
        }).await.unwrap();

        SerializableResource::Raw(on_done_rx.await.unwrap())
    }.boxed()))).as_u64()
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

    caller.data_mut().resource_add(Resource::KvGetResult(FutureResource::for_future(async move {
        let (result_tx, result_rx) = oneshot::channel();

        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Get { key, result: result_tx },
        }).await.unwrap();

        SerializableResource::Raw(match result_rx.await.unwrap() {
            None => KvGetResponse::KeyNotFound,
            Some(v) => KvGetResponse::Ok(v),
        })
    }.boxed()))).as_u64()
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

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        kv_tx.send_async(KvMessage {
            namespace,
            operation: KvOperation::Delex(KvDelexRequest { key, ifeq }, result_tx),
        }).await.unwrap();

        result_rx.await.unwrap()
    }.boxed())).as_u64()
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

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        result_rx.await.unwrap();
    }.boxed())).as_u64()
}

pub(crate) fn fx_tasks_background_spawn_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, function_resource_id: u64) {
    let resource = FunctionResourceId::new(function_resource_id);
    caller.data_mut().tasks_background.push(resource);
}
