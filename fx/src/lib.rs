pub use {
    fx_core::{
        HttpRequest,
        HttpResponse,
        SqlQuery,
        DatabaseSqlQuery,
        DatabaseSqlBatchQuery,
        SqlResult,
        SqlValue,
        FxExecutionError,
        FxFutureError,
        FxSqlError,
        QueueMessage,
    },
    fx_macro::rpc,
    futures::FutureExt,
    crate::{
        sys::PtrWithLen,
        fx_futures::FxFuture,
        fx_streams::{FxStream, FxStreamExport, FxStreamImport},
        error::FxError,
        http::{FxHttpRequest, fetch},
    },
};

use {
    std::{sync::{atomic::{AtomicBool, Ordering}, Once}, panic, time::Duration, ops::Sub},
    lazy_static::lazy_static,
    thiserror::Error,
    chrono::{DateTime, Utc, TimeZone},
    crate::{sys::read_memory, logging::FxLoggingLayer, fx_futures::{FxHostFuture, PoolIndex}},
};

pub mod utils;

mod error;
mod fx_futures;
mod fx_streams;
mod http;
mod sys;
mod logging;

lazy_static! {
    pub static ref CTX: FxCtx = FxCtx::new();
}

pub struct FxCtx {
    logger_init: AtomicBool,
}

impl FxCtx {
    pub fn new() -> Self {
        Self {
            logger_init: AtomicBool::new(false),
        }
    }

    pub fn init_logger(&self) {
        if self.logger_init.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return;
        }

        use tracing_subscriber::prelude::*;
        tracing::subscriber::set_global_default(tracing_subscriber::Registry::default().with(FxLoggingLayer)).unwrap();
    }

    pub fn kv(&self, namespace: impl Into<String>) -> KvStore {
        KvStore::new(namespace)
    }

    pub fn sql(&self, name: impl Into<String>) -> SqlDatabase {
        SqlDatabase::new(name.into())
    }

    pub fn queue(&self, name: impl Into<String>) -> Queue {
        Queue::new(name.into())
    }

    pub async fn rpc<T: serde::ser::Serialize, R: serde::de::DeserializeOwned>(&self, service_id: impl Into<String>, function: impl Into<String>, arg: T) -> Result<R, FxFutureError> {
        let service_id = service_id.into();
        let service_id = service_id.as_bytes();
        let function = function.into();
        let function = function.as_bytes();
        let arg = rmp_serde::to_vec(&arg).unwrap();
        let arg = arg.as_slice();

        let future_index = unsafe {
            sys::rpc(
                service_id.as_ptr() as i64,
                service_id.len() as i64,
                function.as_ptr() as i64,
                function.len() as i64,
                arg.as_ptr() as i64,
                arg.len() as i64,
            )
        };

        let response = FxHostFuture::new(PoolIndex(future_index as u64)).await?;
        Ok(rmp_serde::from_slice(&response).unwrap())
    }

    pub fn rpc_async<T: serde::ser::Serialize>(&self, service_id: impl Into<String>, function: impl Into<String>, arg: T) {
        let service_id = service_id.into();
        let service_id = service_id.as_bytes();
        let function = function.into();
        let function = function.as_bytes();
        let arg = rmp_serde::to_vec(&arg).unwrap();
        let arg = arg.as_slice();

        unsafe {
            sys::rpc_async(
                service_id.as_ptr() as i64,
                service_id.len() as i64,
                function.as_ptr() as i64,
                function.len() as i64,
                arg.as_ptr() as i64,
                arg.len() as i64
            );
        }
    }

    pub fn random(&self, len: u64) -> Vec<u8> {
        let ptr_and_len = sys::PtrWithLen::new();
        unsafe { sys::random(len as i64, ptr_and_len.ptr_to_self()) };
        ptr_and_len.read_owned()
    }

    pub fn now(&self) -> FxInstant {
        FxInstant::now()
    }
}

pub struct KvStore {
    binding: String,
}

impl KvStore {
    pub(crate) fn new(binding: impl Into<String>) -> Self {
        Self {
            binding: binding.into(),
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, KvError> {
        let key = key.as_bytes();
        let ptr_and_len = sys::PtrWithLen::new();
        let result = unsafe { sys::kv_get(self.binding.as_ptr() as i64, self.binding.len() as i64, key.as_ptr() as i64, key.len() as i64, ptr_and_len.ptr_to_self()) };
        match result {
            0 => Ok(Some(ptr_and_len.read_owned())),
            1 => Err(KvError::BindingDoesNotExist),
            2 => Ok(None),
            _ => Err(KvError::UnknownError),
        }
    }

    pub fn set(&self, key: &str, value: &[u8]) -> Result<(), KvError> {
        let key = key.as_bytes();
        let result = unsafe { sys::kv_set(self.binding.as_ptr() as i64, self.binding.len() as i64, key.as_ptr() as i64, key.len() as i64, value.as_ptr() as i64, value.len() as i64) };
        match result {
            0 => Ok(()),
            1 => Err(KvError::BindingDoesNotExist),
            _ => Err(KvError::UnknownError),
        }
    }
}

#[derive(Error, Debug, Eq, PartialEq)]
pub enum KvError {
    #[error("binding does not exist")]
    BindingDoesNotExist,

    #[error("unknown error")]
    UnknownError,
}

#[derive(Clone, Debug)]
pub struct SqlDatabase {
    name: String,
}

impl SqlDatabase {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    pub fn exec(&self, query: SqlQuery) -> Result<SqlResult, FxSqlError> {
        let query = DatabaseSqlQuery {
            database: self.name.clone(),
            query,
        };
        let query = rmp_serde::to_vec(&query).unwrap();
        let ptr_and_len = sys::PtrWithLen::new();
        unsafe {
            sys::sql_exec(query.as_ptr() as i64, query.len() as i64, ptr_and_len.ptr_to_self())
        }

        rmp_serde::from_slice(&ptr_and_len.read_owned()).unwrap()
    }

    pub fn batch(&self, queries: Vec<SqlQuery>) {
        let queries = DatabaseSqlBatchQuery {
            database: self.name.clone(),
            queries,
        };
        let queries = rmp_serde::to_vec(&queries).unwrap();
        unsafe { sys::sql_batch(queries.as_ptr() as i64, queries.len() as i64); }
    }
}

#[derive(Clone)]
pub struct Queue {
    queue_name: String,
}

impl Queue {
    pub fn new(queue_name: String) -> Self {
        Self { queue_name }
    }

    pub fn push<T: serde::ser::Serialize>(&self, argument: T) {
        self.push_raw(rmp_serde::to_vec(&argument).unwrap());
    }

    pub fn push_raw(&self, argument: Vec<u8>) {
        unsafe {
            sys::queue_push(self.queue_name.as_ptr() as i64, self.queue_name.len() as i64, argument.as_ptr() as i64, argument.len() as i64);
        }
    }
}

pub async fn sleep(duration: Duration) {
    FxHostFuture::new(PoolIndex(unsafe { sys::sleep(duration.as_millis() as i64) } as u64)).await.unwrap();
}

pub fn read_rpc_request<T: serde::de::DeserializeOwned>(addr: i64, len: i64) -> Result<T, FxError> {
    rmp_serde::from_slice(read_memory(addr, len))
        .map_err(|err| FxError::DeserializationError { reason: format!("failed to deserialize: {err:?}") })
}

pub fn write_rpc_response<T: serde::ser::Serialize>(response: T) {
    write_rpc_response_raw(rmp_serde::to_vec(&response).unwrap());
}

pub fn write_rpc_response_raw(response: Vec<u8>) {
    unsafe { sys::send_rpc_response(response.as_ptr() as i64, response.len() as i64) };
}

pub fn write_error(error: FxExecutionError) {
    let error = rmp_serde::to_vec(&error).unwrap();
    unsafe { sys::send_error(error.as_ptr() as i64, error.len() as i64); }
}

pub fn panic_hook(info: &panic::PanicHookInfo) {
    let payload = info.payload().downcast_ref::<&str>()
        .map(|v| v.to_owned().to_owned())
        .or(info.payload().downcast_ref::<String>().map(|v| v.to_owned()));
    tracing::error!("fx module panic: {info:?}, payload: {payload:?}");
}

pub fn set_panic_hook() {
    static SET_HOOK: Once = Once::new();
    SET_HOOK.call_once(|| { std::panic::set_hook(Box::new(panic_hook)); });
}

pub fn to_vec<T: serde::ser::Serialize>(v: T) -> Vec<u8> {
    rmp_serde::to_vec(&v).unwrap()
}

pub struct FxInstant {
    millis_since_unix: i64,
}

impl FxInstant {
    pub fn now() -> Self {
        Self {
            millis_since_unix: unsafe { sys::time() },
        }
    }

    pub fn to_datetime(&self) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(self.millis_since_unix).single().unwrap()
    }
}

impl Sub<FxInstant> for FxInstant {
    type Output = Duration;
    fn sub(self, rhs: FxInstant) -> Self::Output {
        Duration::from_millis((self.millis_since_unix - rhs.millis_since_unix) as u64)
    }
}
