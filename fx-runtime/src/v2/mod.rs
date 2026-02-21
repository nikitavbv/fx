use {
    std::{
        io::Cursor,
        path::{PathBuf, Path},
        collections::HashMap,
        net::SocketAddr,
        convert::Infallible,
        pin::Pin,
        rc::Rc,
        cell::{RefCell, Cell},
        task::Poll,
        thread::JoinHandle,
        fmt::Debug,
        marker::PhantomData,
        time::{Duration, SystemTime, UNIX_EPOCH},
        sync::{Arc, RwLock},
    },
    tracing::{Level, info, error, warn},
    tracing_subscriber::FmtSubscriber,
    tokio::{fs, sync::oneshot},
    clap::{Parser, Subcommand, ValueEnum, builder::PossibleValue},
    ::futures::{FutureExt, StreamExt, future::BoxFuture, future::LocalBoxFuture, stream::FuturesUnordered},
    hyper::{Response, body::Bytes, server::conn::http1, StatusCode},
    hyper_util::rt::{TokioIo, TokioTimer},
    http_body_util::{Full, BodyStream},
    walkdir::WalkDir,
    thiserror::Error,
    notify::Watcher,
    wasmtime::{AsContext, AsContextMut},
    futures_intrusive::sync::LocalMutex,
    slotmap::{SlotMap, Key as SlotMapKey},
    rand::TryRngCore,
    axum::{Router, routing::{get, delete}, response::Response as AxumResponse, Extension, extract},
    leptos::prelude::*,
    serde::{Serialize, Deserialize},
    ::http::Method,
    fx_types::{
        capnp,
        abi_function_resources_capnp,
        abi_log_capnp,
        abi_sql_capnp,
        abi_blob_capnp,
        abi_http_capnp,
        abi_metrics_capnp,
        abi::FuturePollResult,
    },
    self::{
        definitions::DefinitionsMonitor,
        config::{FunctionConfig, FunctionCodeConfig, LoggerConfig},
        errors::{FunctionFuturePollError, FunctionFutureError, FunctionDeploymentHandleRequestError},
        function::{FunctionDeploymentId, FunctionInstanceState, FunctionInstance},
        sql::{Value as SqlValue, Row as SqlRow},
        logs::{LogMessageEvent, LogEventLevel, BoxLogger, StdoutLogger, NoopLogger, Logger, EventFieldValue, LogSource},
    },
};

pub use self::{runtime::FxServerV2, config::ServerConfig, function::FunctionId};

pub mod config;
mod cron;
mod definitions;
mod http;
pub mod logs;
mod function;
mod errors;
pub mod runtime;
mod sql;

#[derive(Debug)]
enum WorkerMessage {
    RemoveFunction {
        function_id: FunctionId,
        on_ready: Option<oneshot::Sender<()>>,
    },
    FunctionDeploy {
        function_id: FunctionId,
        deployment_id: FunctionDeploymentId,
        module: wasmtime::Module,

        http_listeners: Vec<FunctionHttpListener>,

        bindings_sql: HashMap<String, SqlBindingConfig>,
        bindings_blob: HashMap<String, BlobBindingConfig>,
    },
}

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

#[derive(Debug)]
struct MetricsFlushMessage {
    function_metrics: HashMap<FunctionId, FunctionMetricsDelta>,
}

#[derive(Debug)]
struct FunctionMetricsDelta {
    counters_delta: HashMap<MetricKey, u64>,
}

impl FunctionMetricsDelta {
    pub fn empty() -> Self {
        Self {
            counters_delta: HashMap::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.counters_delta.is_empty()
    }

    fn append(&mut self, other: FunctionMetricsDelta) {
        for (metric_key, delta) in other.counters_delta {
            *self.counters_delta.entry(metric_key).or_insert(0) += delta;
        }
    }
}

struct DebugWrapper<T>(T);

impl<T> DebugWrapper<T> {
    fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Debug for DebugWrapper<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        "DebugWrapper<T>".fmt(f)
    }
}

impl<T> AsRef<T> for DebugWrapper<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

struct CompilerMessage {
    function_id: FunctionId,
    code: Vec<u8>,
    response: oneshot::Sender<wasmtime::Module>,
}

enum ManagementMessage {
    DeployFunction(DeployFunctionMessage),
    WorkerMetrics(MetricsFlushMessage),
}

struct DeployFunctionMessage {
    function_id: FunctionId,
    function_config: FunctionConfig,
    on_ready: oneshot::Sender<()>,
}

enum FunctionResponseHttpBodyInner {
    Full(RefCell<Option<Bytes>>),
    FunctionResource(RefCell<FunctionResourceReader>),
}

enum FunctionResourceReader {
    Empty,
    Resource(SerializedFunctionResource<Vec<u8>>),
    Future(LocalBoxFuture<'static, Vec<u8>>),
}

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

pub struct FetchRequestHeader {
    inner: ::http::request::Parts,
    body_resource_id: Option<ResourceId>, // TODO: drop body if FetchRequestHeader is dropped without consumption
}

impl FetchRequestHeader {
    fn uri(&self) -> &::http::Uri {
        &self.inner.uri
    }

    fn method(&self) -> &::http::Method {
        &self.inner.method
    }

    fn headers(&self) -> &::http::HeaderMap {
        &self.inner.headers
    }
}

impl From<::http::request::Parts> for FetchRequestHeader {
    fn from(value: ::http::request::Parts) -> Self {
        Self {
            inner: value,
            body_resource_id: None,
        }
    }
}

pub struct FetchRequestBody(FetchRequestBodyInner);

impl From<hyper::body::Incoming> for FetchRequestBody {
    fn from(value: hyper::body::Incoming) -> Self {
        Self(FetchRequestBodyInner::Stream(Box::pin(value)))
    }
}

enum FetchRequestBodyInner {
    // full body:
    Full(Vec<u8>),
    FullSerialized(Vec<u8>),
    // streaming body:
    Stream(Pin<Box<hyper::body::Incoming>>),
    PartiallyReadStream {
        stream: Pin<Box<hyper::body::Incoming>>,
        frame: Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>,
    },
    PartiallyReadStreamSerialized {
        stream: Pin<Box<hyper::body::Incoming>>,
        frame_serialized: Vec<u8>,
    },
}

struct FunctionResponse(FunctionResponseInner);

enum FunctionResponseInner {
    HttpResponse(FunctionHttpResponse),
}

struct FunctionHttpResponse {
    status: ::http::status::StatusCode,
    body: Cell<Option<SerializedFunctionResource<Vec<u8>>>>,
}

/// Resource that origins from function side and is not owned by host.
/// moved lazily from function to host memory.
/// if dropped before being moved, cleans up resource on function side.
struct SerializedFunctionResource<T: DeserializeFunctionResource> {
    _t: PhantomData<T>,
    resource: OwnedFunctionResourceId,
}

impl<T: DeserializeFunctionResource> SerializedFunctionResource<T> {
    pub fn new(instance: Rc<FunctionInstance>, resource: FunctionResourceId) -> Self {
        Self {
            _t: PhantomData,
            resource: OwnedFunctionResourceId::new(instance, resource),
        }
    }

    async fn move_to_host(self) -> T {
        let (instance, resource) = self.resource.consume();
        T::deserialize(&mut instance.move_serializable_resource_to_host(&resource).await.as_slice(), instance)
    }
}

/// Function resource handle that is owned by host.
/// Cleans up function memory if dropped before being consumed
pub struct OwnedFunctionResourceId(Cell<Option<(Rc<FunctionInstance>, FunctionResourceId)>>);

impl OwnedFunctionResourceId {
    pub fn new(function_instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self(Cell::new(Some((function_instance, resource_id))))
    }

    pub fn consume(self) -> (Rc<FunctionInstance>, FunctionResourceId) {
        self.0.replace(None).unwrap()
    }
}

impl Drop for OwnedFunctionResourceId {
    fn drop(&mut self) {
        if let Some((function_instance, resource_id)) = self.0.replace(None) {
            tokio::task::spawn_local(async move {
                function_instance.resource_drop(&resource_id).await;
            });
        }
    }
}

trait DeserializeFunctionResource {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self;
}

impl DeserializeFunctionResource for FunctionResponse {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let response = message_reader.get_root::<abi_function_resources_capnp::function_response::Reader>().unwrap();
        Self(FunctionResponseInner::HttpResponse(FunctionHttpResponse {
            status: ::http::StatusCode::from_u16(response.get_status()).unwrap(),
            body: Cell::new(Some(SerializedFunctionResource::new(instance, FunctionResourceId::from(response.get_body_resource())))),
        }))
    }
}

impl DeserializeFunctionResource for Vec<u8> {
    fn deserialize(resource: &mut &[u8], _instance: Rc<FunctionInstance>) -> Self {
        resource.to_vec()
    }
}

struct ResourceId {
    id: u64,
}

impl ResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<slotmap::DefaultKey> for ResourceId {
    fn from(value: slotmap::DefaultKey) -> Self {
        Self::new(value.data().as_ffi())
    }
}

impl Into<slotmap::DefaultKey> for &ResourceId {
    fn into(self) -> slotmap::DefaultKey {
        slotmap::DefaultKey::from(slotmap::KeyData::from_ffi(self.id))
    }
}

impl From<u64> for ResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

#[derive(Clone, Debug)]
struct FunctionResourceId {
    id: u64,
}

impl FunctionResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<u64> for FunctionResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

enum Resource {
    FetchRequest(SerializableResource<FetchRequestHeader>),
    RequestBody(FetchRequestBody),
    SqlQueryResult(FutureResource<SerializableResource<SqlQueryResult>>),
    SqlMigrationResult(FutureResource<SerializableResource<SqlMigrationResult>>),
    UnitFuture(BoxFuture<'static, ()>),
    BlobGetResult(FutureResource<SerializableResource<BlobGetResponse>>),
    FetchResult(FutureResource<SerializableResource<FetchResult>>),
}

enum SerializableResource<T: SerializeResource> {
    Raw(T),
    Serialized(Vec<u8>),
}

impl<T: SerializeResource> SerializableResource<T> {
    fn map_to_serialized(self) -> Self {
        match self {
            Self::Raw(t) => Self::Serialized(t.serialize()),
            Self::Serialized(v) => Self::Serialized(v),
        }
    }

    fn serialized_size(&self) -> usize {
        match self {
            Self::Raw(_) => panic!("cannot compute serialized size for resource that is not serialized yet"),
            Self::Serialized(v) => v.len(),
        }
    }

    fn into_serialized(self) -> Vec<u8> {
        match self {
            Self::Raw(t) => t.serialize(),
            Self::Serialized(v) => v,
        }
    }
}

trait SerializeResource {
    fn serialize(self) -> Vec<u8>;
}

impl SerializeResource for FetchRequestHeader {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut resource = message.init_root::<abi_http_capnp::http_request::Builder>();

        resource.set_uri(self.uri().to_string());
        resource.set_method(match &*self.method() {
            &hyper::Method::GET => abi_http_capnp::HttpMethod::Get,
            &hyper::Method::POST => abi_http_capnp::HttpMethod::Post,
            &hyper::Method::PUT => abi_http_capnp::HttpMethod::Put,
            &hyper::Method::PATCH => abi_http_capnp::HttpMethod::Patch,
            &hyper::Method::DELETE => abi_http_capnp::HttpMethod::Delete,
            &hyper::Method::OPTIONS => abi_http_capnp::HttpMethod::Options,
            other => todo!("this http method not supported: {other:?}"),
        });

        let mut request_headers = resource.reborrow().init_headers(self.headers().len() as u32);
        for (index, (header_name, header_value)) in self.headers().iter().enumerate() {
            let mut request_header = request_headers.reborrow().get(index as u32);
            request_header.set_name(header_name.as_str());
            request_header.set_value(header_value.to_str().unwrap());
        }

        let mut resource_body = resource.init_body().init_body();
        match self.body_resource_id {
            None => resource_body.set_empty(()),
            Some(resource_id) => resource_body.set_host_resource(resource_id.as_u64()),
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

impl SerializeResource for SqlQueryResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let sql_exec_response = message.init_root::<abi_sql_capnp::sql_exec_result::Builder>();
        let sql_exec_response = sql_exec_response.init_result();

        match self {
            Self::Ok(rows) => {
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
            Self::Error(err) => {
                let mut response_error = sql_exec_response.init_error();
                match err {
                    SqlQueryExecutionError::DatabaseBusy => response_error.set_database_busy(()),
                }
            }
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

impl SerializeResource for SqlMigrationResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let sql_migrate_result = message.init_root::<abi_sql_capnp::sql_migrate_result::Builder>();
        let mut sql_migrate_result = sql_migrate_result.init_result();

        match self {
            Self::Ok(_) => {
                sql_migrate_result.set_ok(());
            },
            Self::Error(err) => {
                let mut response_error = sql_migrate_result.init_error();
                match err {
                    SqlMigrationError::DatabaseBusy => response_error.set_database_busy(()),
                }
            }
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

impl SerializeResource for Vec<u8> {
    fn serialize(self) -> Vec<u8> {
        self
    }
}

enum FutureResource<T> {
    Future(BoxFuture<'static, T>),
    Ready(T),
}

struct FunctionFuture {
    inner: LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>>,
    instance: Rc<FunctionInstance>,
    resource_id: FunctionResourceId,
}

impl FunctionFuture {
    fn new(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self {
            inner: Self::start_new_poll_call(instance.clone(), resource_id.clone()),
            instance,
            resource_id,
        }
    }

    fn start_new_poll_call(instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> LocalBoxFuture<'static, Result<Poll<()>, FunctionFuturePollError>> {
        async move {
            let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
            instance.future_poll(&resource_id, waker).await
        }.boxed_local()
    }
}

impl Future for FunctionFuture {
    type Output = Result<FunctionResourceId, FunctionFutureError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        match self.inner.poll_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(v) => {
                let poll = match v {
                    Ok(v) => v,
                    Err(err) => return Poll::Ready(Err(match err {
                        FunctionFuturePollError::FunctionPanicked => FunctionFutureError::FunctionPanicked,
                    })),
                };

                match poll {
                    Poll::Pending => {
                        self.inner = Self::start_new_poll_call(self.instance.clone(), self.resource_id.clone());
                        Poll::Pending
                    },
                    Poll::Ready(_) => Poll::Ready(Ok(self.resource_id.clone()))
                }
            },
        }
    }
}

enum BlobGetResponse {
    NotFound,
    Ok(Vec<u8>),
    BindingNotExists,
}

impl SerializeResource for BlobGetResponse {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let blob_get_response = message.init_root::<abi_blob_capnp::blob_get_response::Builder>();
        let mut response = blob_get_response.init_response();

        match self {
            Self::NotFound => response.set_not_found(()),
            Self::Ok(v) => response.set_value(&v),
            Self::BindingNotExists => response.set_binding_not_exists(()),
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

struct FetchResult {
    status: ::http::StatusCode,
    body: Vec<u8>,
}

impl FetchResult {
    pub fn new(status: ::http::StatusCode, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
        }
    }
}

impl SerializeResource for FetchResult {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut fetch_response = message.init_root::<abi_http_capnp::http_response::Builder>();

        fetch_response.set_status(self.status.as_u16());
        fetch_response.set_body(&self.body);

        capnp::serialize::write_message_to_words(&message)
    }
}

fn serialize_request_body_full(body: Vec<u8>) -> Vec<u8> {
    todo!("serialize request body full")
}

fn serialize_partially_read_stream(frame: Option<Result<hyper::body::Frame<Bytes>, hyper::Error>>) -> Vec<u8> {
    let mut message = capnp::message::Builder::new_default();
    let serialized_frame = message.init_root::<abi_http_capnp::http_request_body_frame::Builder>();
    let mut serialized_frame = serialized_frame.init_body();

    match frame {
        None => serialized_frame.set_stream_end(()),
        Some(Err(err)) => todo!("handle error: {err:?}"),
        Some(Ok(frame)) => serialized_frame.set_bytes(&frame.into_data().unwrap()),
    }

    capnp::serialize::write_message_to_words(&message)
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionHttpListener {
    pub(crate) host: Option<String>,
}

impl FunctionHttpListener {
    fn new(host: Option<String>) -> Self {
        Self {
            host,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SqlBindingConfig {
    pub(crate) connection_id: String,
    pub(crate) location: SqlBindingConfigLocation,
    pub(crate) busy_timeout: Option<Duration>,
}

#[derive(Debug, Clone)]
enum SqlBindingConfigLocation {
    InMemory(String),
    Path(PathBuf),
}

#[derive(Debug, Clone)]
struct BlobBindingConfig {
    storage_directory: PathBuf,
}

fn create_logger(logger: &LoggerConfig) -> BoxLogger {
    match logger {
        LoggerConfig::Stdout => BoxLogger::new(StdoutLogger::new()),
        LoggerConfig::Noop => BoxLogger::new(NoopLogger::new()),
        LoggerConfig::HttpLogger { endpoint } => BoxLogger::new(HttpLogger::new(endpoint.clone())),
        LoggerConfig::Custom(v) => BoxLogger::new(v.clone()),
    }
}

struct HttpLogger {
    client: reqwest::blocking::Client,
    endpoint: String,
}

impl HttpLogger {
    pub fn new(endpoint: String) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            endpoint,
        }
    }
}

impl Logger for HttpLogger {
    fn log(&self, message: LogMessageEvent) {
        let response = self.client.post(&self.endpoint)
            .header("content-type", "application/stream+json")
            .body(serde_json::to_vec(&HttpLogEvent {
                source: "fx".to_owned(),
                function_id: match message.source {
                    LogSource::FxRuntime => None,
                    LogSource::Function { id } => Some(id),
                },
                message: message.fields.get("message")
                    .map(|v| match v {
                        EventFieldValue::Text(v) => v.clone(),
                        other => format!("{other:?}"),
                    })
                    .unwrap_or(format!("fx log")),
                request_id: message.fields.get("request_id")
                    .map(|v| match v {
                        EventFieldValue::Text(v) => v.clone(),
                        other => format!("{other:?}"),
                    }),
                level: message.level,
                timestamp: chrono::Utc::now().to_rfc3339(),
                fields: message.fields.into_iter()
                    .map(|(key, value)| (
                        key.clone(),
                        map_log_value_to_serde_json(&value)
                    ))
                    .collect()
            }).unwrap())
            .send()
            .unwrap();
        if !response.status().is_success() {
            error!(status_code=response.status().as_u16(), "failed to send logs over http");
        }
    }
}

fn map_log_value_to_serde_json(value: &EventFieldValue) -> serde_json::Value {
    match value {
        EventFieldValue::Text(v) => serde_json::Value::String(v.clone()),
        EventFieldValue::F64(v) => serde_json::Value::Number(serde_json::Number::from_f64(*v).unwrap()),
        EventFieldValue::I64(v) => serde_json::Value::Number((*v).into()),
        EventFieldValue::U64(v) => serde_json::Value::Number((*v).into()),
        EventFieldValue::Object(v) => serde_json::Value::Object(
            v.iter()
                .map(|(key, value)| (key.clone(), map_log_value_to_serde_json(value)))
                .collect()
        )
    }
}

#[derive(Serialize)]
struct HttpLogEvent {
    fields: HashMap<String, serde_json::Value>,
    level: LogEventLevel,
    message: String,
    request_id: Option<String>,
    source: String,
    function_id: Option<String>,
    timestamp: String,
}

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
struct MetricKey {
    name: String,
    labels: Vec<(String, String)>,
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct MetricId {
    id: u64,
}

impl MetricId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn from_abi(id: u64) -> Self {
        Self::new(id)
    }

    pub fn into_abi(&self) -> u64 {
        self.id
    }
}

struct FunctionMetricsState {
    metrics_series: Vec<MetricKey>,
    metric_key_to_ids: HashMap<MetricKey, MetricId>,
    metrics_counter_delta: HashMap<MetricId, u64>,
}

impl FunctionMetricsState {
    pub fn new() -> Self {
        Self {
            metrics_series: Vec::new(),
            metric_key_to_ids: HashMap::new(),
            metrics_counter_delta: HashMap::new(),
        }
    }

    fn counter_register(&mut self, key: MetricKey) -> MetricId {
        *self.metric_key_to_ids.entry(key.clone()).or_insert_with(|| {
            let index = self.metrics_series.len() as u64;
            self.metrics_series.push(key);
            MetricId::new(index)
        })
    }

    fn counter_increment(&mut self, key: MetricId, delta: u64) {
        *self.metrics_counter_delta.entry(key).or_insert(0) += delta;
    }

    fn flush_delta(&mut self) -> FunctionMetricsDelta {
        let counters_delta = std::mem::replace(&mut self.metrics_counter_delta, HashMap::new())
            .into_iter()
            .map(|(metric_id, delta)| (self.metrics_series.get(metric_id.id as usize).unwrap().clone(), delta))
            .collect();

        FunctionMetricsDelta {
            counters_delta,
        }
    }
}

struct MetricsRegistry {
    function_metrics: RwLock<HashMap<FunctionId, FunctionMetricsDelta>>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            function_metrics: RwLock::new(HashMap::new()),
        }
    }

    pub fn update(&self, function_metrics: HashMap<FunctionId, FunctionMetricsDelta>) {
        let mut all_metrics = self.function_metrics.write().unwrap();

        for (function_id, metrics) in function_metrics {
            all_metrics.entry(function_id)
                .or_insert_with(|| FunctionMetricsDelta::empty())
                .append(metrics);
        }
    }

    pub fn encode(&self) -> String {
        let mut result = String::new();
        let all_metrics = self.function_metrics.read().unwrap();

        for (function_id, function_metrics) in all_metrics.iter() {
            let function_id = sanitize_metric_name(function_id.as_str());

            for (counter_key, counter_value) in function_metrics.counters_delta.iter() {
                let metric_name = {
                    let mut metric_name = String::new();
                    metric_name.push_str("function_");
                    metric_name.push_str(sanitize_metric_name(function_id.as_str()).as_str());
                    metric_name.push('_');
                    metric_name.push_str(sanitize_metric_name(counter_key.name.as_str()).as_str());
                    metric_name
                };

                result.push_str("# TYPE ");
                result.push_str(metric_name.as_str());
                result.push_str(" counter\n");
                result.push_str(metric_name.as_str());

                if !counter_key.labels.is_empty() {
                    result.push('{');

                    for (index, (label_key, label_value)) in counter_key.labels.iter().enumerate() {
                        if index > 0 {
                            result.push(',');
                        }
                        result.push_str(sanitize_label_name(label_key.as_str()).as_str());
                        result.push_str("=\"");
                        result.push_str(escape_label_value(label_value.as_str()).as_str());
                        result.push('"');
                    }

                    result.push('}')
                }

                result.push(' ');
                result.push_str(counter_value.to_string().as_str());
                result.push('\n');
            }
        }

        result
    }
}

fn sanitize_metric_name(s: &str) -> String {
    s.chars()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                c
            } else if i == 0 && c.is_ascii_digit() {
                '_'
            } else {
                '_'
            }
        })
        .collect()
}

fn sanitize_label_name(s: &str) -> String {
    s.chars()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_alphabetic() || c == '_' || (i > 0 && c.is_ascii_digit()) { c } else { '_' }
        })
        .collect()
}

fn escape_label_value(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace('"', "\\\"")
     .replace('\n', "\\n")
}

async fn run_introspection_server(metrics: Arc<MetricsRegistry>, workers_controller: WorkersController) {
    let app = Router::new()
        .route("/", get(introspection_home))
        .route("/metrics", get(introspection_metrics))
        .route("/api/functions/{function_id}", delete(management_api_function_remove))
        .layer(Extension(metrics))
        .layer(Extension(Arc::new(workers_controller)));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn introspection_home() -> AxumResponse {
    render_component(view! {
        <>
            <h2>"fx runtime"</h2>
            <br></br>
            <a href="/introspection">"/introspection"</a>" - realtime dashboard for troubleshooting and insights."<br></br>
            <a href="/metrics">"/metrics"</a>" - metrics exported in prometheus format."<br></br>
        </>
    })
}

async fn introspection_metrics(Extension(metrics): Extension<Arc<MetricsRegistry>>) -> String {
    metrics.encode()
}

fn render_component(component: impl IntoView + 'static) -> AxumResponse {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(component.to_html().into())
        .unwrap()
}

#[derive(Deserialize)]
struct FunctionIdPathArgument {
    function_id: String,
}

async fn management_api_function_remove(
    Extension(workers_controller): Extension<Arc<WorkersController>>,
    extract::Path(function_id): extract::Path<FunctionIdPathArgument>
) -> &'static str {
    workers_controller.function_remove(&FunctionId::new(&function_id.function_id)).await;
    "ok.\n"
}

struct WorkersController {
    workers_tx: Vec<flume::Sender<WorkerMessage>>,
}

impl WorkersController {
    pub fn new(workers_tx: Vec<flume::Sender<WorkerMessage>>) -> Self {
        Self {
            workers_tx,
        }
    }

    async fn function_remove(&self, function_id: &FunctionId) {
        let subtasks = FuturesUnordered::new();

        for worker in &self.workers_tx {
            subtasks.push(async {
                let (on_ready_tx, on_ready_rx) = oneshot::channel();
                worker.send_async(WorkerMessage::RemoveFunction {
                    function_id: function_id.clone(),
                    on_ready: Some(on_ready_tx),
                }).await.unwrap();
                on_ready_rx.await.unwrap();
            });
        }

        subtasks.collect::<Vec<_>>().await;
    }
}
