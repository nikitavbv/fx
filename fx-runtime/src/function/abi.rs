fn fx_log_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: i64, req_len: i64) {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let context = caller.as_context();
    let view = memory.data(&context);

    let mut message_bytes = &view[req_addr as usize..(req_addr + req_len) as usize];
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes, capnp::message::ReaderOptions::default()).unwrap();
    let message = message_reader.get_root::<abi_log_capnp::log_message::Reader>().unwrap();

    let message: LogMessageEvent = LogMessageEvent::new(
        logs::LogSource::function(&caller.data().function_id),
        match message.get_event_type().unwrap() {
            abi_log_capnp::EventType::Begin => logs::LogEventType::Begin,
            abi_log_capnp::EventType::End => logs::LogEventType::End,
            abi_log_capnp::EventType::Instant => logs::LogEventType::Instant,
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

fn fx_resource_serialize_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) -> u64 {
    caller.data_mut().resource_serialize(&ResourceId::from(resource_id)) as u64
}

fn fx_resource_move_from_host_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) {
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
        Resource::UnitFuture(_) => panic!("unit future cannot be moved to function"),
        Resource::BlobGetResult(res) => match res {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::FetchResult(res) => match res {
            FutureResource::Future(_) => panic!("cannot move resource that is not ready yet"),
            FutureResource::Ready(v) => v.into_serialized(),
        },
        Resource::RequestBody(_) => panic!("resource of this type cannot be moved"),
    };

    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;

    view[ptr..ptr+resource.len()].copy_from_slice(&resource);
}

fn fx_resource_drop_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64) {
    let _ = caller.data_mut().resource_remove(&ResourceId::from(resource_id));
}

fn fx_stream_frame_read_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, resource_id: u64, ptr: u64) {
    let serialized_frame = caller.data_mut().stream_read_frame(&ResourceId::from(resource_id));

    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;

    view[ptr..ptr+serialized_frame.len()].copy_from_slice(&serialized_frame);
}

fn fx_sql_exec_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: u64, req_len: u64) -> u64 {
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

    let binding = caller.data().bindings_sql.get(message.get_binding().unwrap().to_str().unwrap()).unwrap();

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

    caller.data_mut().resource_add(Resource::SqlQueryResult(FutureResource::Future(async move {
        SerializableResource::Raw(response_rx.await.unwrap())
    }.boxed()))).as_u64()
}

fn fx_sql_migrate_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_addr: u64, req_len: u64) -> u64 {
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

    let binding = caller.data().bindings_sql.get(message.get_binding().unwrap().to_str().unwrap()).unwrap();

    let (response_tx, response_rx) = oneshot::channel();
    caller.data().sql_tx.send(SqlMessage::Migrate(SqlMigrateMessage {
        binding: binding.clone(),
        migrations: message.get_migrations().unwrap().into_iter()
            .map(|v| v.unwrap().to_string().unwrap())
            .collect(),
        response: response_tx,
    })).unwrap();

    caller.data_mut().resource_add(Resource::SqlMigrationResult(FutureResource::Future(async move {
        SerializableResource::Raw(response_rx.await.unwrap())
    }.boxed()))).as_u64()
}

fn fx_future_poll_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, future_resource_id: u64) -> i64 {
    let resource_id = ResourceId::new(future_resource_id);
    (match caller.data_mut().resource_poll(&resource_id) {
        Poll::Pending => FuturePollResult::Pending,
        Poll::Ready(_) => FuturePollResult::Ready,
    }) as i64
}

fn fx_sleep_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, sleep_millis: u64) -> u64 {
    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        tokio::time::sleep(Duration::from_millis(sleep_millis)).await;
    }.boxed())).as_u64()
}

fn fx_random_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, ptr: u64, len: u64) {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();
    let mut context = caller.as_context_mut();
    let view = memory.data_mut(&mut context);
    let ptr = ptr as usize;
    let len = len as usize;

    rand::rngs::OsRng.try_fill_bytes(&mut view[ptr..ptr+len]).unwrap();
}

fn fx_time_handler(_caller: wasmtime::Caller<'_, FunctionInstanceState>) -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn fx_blob_put_handler(
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
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };

    let binding = caller.data().bindings_blob.get(&binding).unwrap();

    let key = {
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };
    // TODO: add a check to prevent exiting the directory, lol
    let key_path = binding.storage_directory.join(key);

    let value = {
        let ptr = value_ptr as usize;
        let len = value_len as usize;
        view[ptr..ptr+len].to_vec()
    };

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        let parent = key_path.parent().unwrap();
        if !parent.exists() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }

        tokio::fs::write(key_path, value).await.unwrap();
    }.boxed())).as_u64()
}

fn fx_blob_get_handler(
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
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };
    let binding = caller.data().bindings_blob.get(&binding);

    let key_path = binding.map(|v| v.storage_directory.join({
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    }));

    caller.data_mut().resource_add(Resource::BlobGetResult(FutureResource::Future(async move {
        SerializableResource::Raw({
            match key_path {
                Some(v) => match tokio::fs::read(v).await {
                    Ok(v) => BlobGetResponse::Ok(v),
                    Err(err) => {
                        if err.kind() == tokio::io::ErrorKind::NotFound {
                            BlobGetResponse::NotFound
                        } else {
                            todo!("handling for this error kind is not implemented: {err:?}");
                        }
                    }
                },
                None => BlobGetResponse::BindingNotExists
            }
        })
    }.boxed()))).as_u64()
}

fn fx_blob_delete_handler(
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
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };
    let binding = caller.data().bindings_blob.get(&binding).unwrap();

    let key = {
        let ptr = key_ptr as usize;
        let len = key_len as usize;
        String::from_utf8(view[ptr..ptr+len].to_vec()).unwrap()
    };
    let key_path = binding.storage_directory.join(key);

    caller.data_mut().resource_add(Resource::UnitFuture(async move {
        if let Err(err) = tokio::fs::remove_file(&key_path).await {
            if err.kind() != std::io::ErrorKind::NotFound {
                todo!("error handling is not implemented for fx_blob_delete: {:?}", err.kind());
            }
        }
    }.boxed())).as_u64()
}

fn fx_fetch_handler(
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

    let mut fetch_request = reqwest::Request::new(
        match request.get_method().unwrap() {
            abi_http_capnp::HttpMethod::Get => Method::GET,
            abi_http_capnp::HttpMethod::Put => Method::PUT,
            abi_http_capnp::HttpMethod::Post => Method::POST,
            abi_http_capnp::HttpMethod::Patch => Method::PATCH,
            abi_http_capnp::HttpMethod::Delete => Method::DELETE,
            abi_http_capnp::HttpMethod::Options => Method::OPTIONS,
        },
        request.get_uri().unwrap().to_str().unwrap().try_into().unwrap()
    );

    match request.get_body().unwrap().get_body().which().unwrap() {
        abi_http_capnp::http_request_body::body::Which::Empty(_) => {},
        abi_http_capnp::http_request_body::body::Which::Bytes(v) => {
            *fetch_request.body_mut() = Some(reqwest::Body::from(v.unwrap().to_vec()));
        },
        abi_http_capnp::http_request_body::body::Which::HostResource(v) => {
            let resource_id = ResourceId::new(v);
            match caller.data_mut().resource_remove(&resource_id) {
                Resource::BlobGetResult(_)
                | Resource::FetchRequest(_)
                | Resource::FetchResult(_)
                | Resource::SqlQueryResult(_)
                | Resource::SqlMigrationResult(_)
                | Resource::UnitFuture(_) => panic!("this resource cannot be used as request body"),
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
                }
            }
        },
    }

    let client = caller.data().http_client.clone();

    caller.data_mut().resource_add(Resource::FetchResult(FutureResource::Future(async move {
        SerializableResource::Raw({
            let result = client.execute(fetch_request).await.unwrap();
            FetchResult::new(result.status(), result.bytes().await.unwrap().to_vec())
        })
    }.boxed()))).as_u64()
}

fn fx_metrics_counter_register_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, req_ptr: u64, req_len: u64) -> u64 {
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

fn fx_metrics_counter_increment_handler(mut caller: wasmtime::Caller<'_, FunctionInstanceState>, counter_id: u64, delta: u64) {
    caller.data_mut().metrics.counter_increment(MetricId::new(counter_id), delta);
}
