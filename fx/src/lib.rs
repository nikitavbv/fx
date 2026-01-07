pub use {
    fx_common::{
        HttpRequest,
        HttpResponse,
        SqlQuery,
        DatabaseSqlQuery,
        DatabaseSqlBatchQuery,
        SqlValue,
        FxExecutionError,
        FxFutureError,
        FxSqlError,
        QueueMessage,
    },
    fx_macro::rpc,
    futures::FutureExt,
    crate::{
        sys::{PtrWithLen},
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
    fx_api::{capnp, fx_capnp},
    crate::{sys::{read_memory_owned, invoke_fx_api}, logging::FxLoggingLayer, fx_futures::{FxHostFuture, PoolIndex}},
};

pub mod utils;
pub mod metrics;
pub mod sql;

mod api;
mod error;
mod fx_futures;
mod fx_streams;
mod http;
mod invoke;
mod logging;
mod sys;

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

    pub async fn rpc<T: serde::ser::Serialize, R: serde::de::DeserializeOwned>(&self, function_id: impl Into<String>, method: impl Into<String>, arg: T) -> Result<R, FxFutureError> {
        let future_index = {
            let arg = rmp_serde::to_vec(&arg).unwrap();

            let mut message = capnp::message::Builder::new_default();
            let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
            let op = request.init_op();
            let mut rpc_request = op.init_rpc();
            rpc_request.set_function_id(function_id.into());
            rpc_request.set_method_name(method.into());
            rpc_request.set_argument(&arg);
            let response = invoke_fx_api(message);
            let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

            match response.get_op().which().unwrap() {
                fx_capnp::fx_api_call_result::op::Which::Rpc(v) => {
                    let rpc_response = v.unwrap();
                    match rpc_response.get_response().which().unwrap() {
                        fx_capnp::rpc_call_response::response::Which::FutureId(v) => v,
                        _other => panic!("unexpected rpc response"),
                    }
                },
                _other => panic!("unexpected response from rpc api"),
            }
        };

        let response = FxHostFuture::new(PoolIndex(future_index as u64)).await?;
        Ok(rmp_serde::from_slice(&response).unwrap())
    }

    pub fn random(&self, len: u64) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut random_request = op.init_random();
        random_request.set_length(len);

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::Random(v) => v.unwrap().get_data().unwrap().to_vec(),
            _other => panic!("unexpected response from random api"),
        }
    }

    pub fn now(&self) -> FxInstant {
        FxInstant::now()
    }
}

impl Default for FxCtx {
    fn default() -> Self {
        Self::new()
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
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut kv_get_request = op.init_kv_get();
        kv_get_request.set_key(key.as_bytes());
        kv_get_request.set_binding_id(self.binding.as_str());
        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::KvGet(v) => {
                let kv_get_response = v.unwrap();
                match kv_get_response.get_response().which().unwrap() {
                    fx_capnp::kv_get_response::response::Which::BindingNotFound(_) => Err(KvError::BindingDoesNotExist),
                    fx_capnp::kv_get_response::response::Which::KeyNotFound(_) => Ok(None),
                    fx_capnp::kv_get_response::response::Which::Value(v) => Ok(Some(v.unwrap().to_vec())),
                }
            },
            _other => panic!("unexpected response from kv_get api"),
        }
    }

    pub fn set(&self, key: &str, value: &[u8]) -> Result<(), KvError> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut kv_set_request = op.init_kv_set();
        kv_set_request.set_binding_id(self.binding.as_str());
        kv_set_request.set_key(key.as_bytes());
        kv_set_request.set_value(value);
        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::KvSet(v) => {
                let kv_set_response = v.unwrap();
                match kv_set_response.get_response().which().unwrap() {
                    fx_capnp::kv_set_response::response::Which::BindingNotFound(_) => Err(KvError::BindingDoesNotExist),
                    fx_capnp::kv_set_response::response::Which::Ok(_) => Ok(()),
                }
            },
            _other => panic!("unexpected response from kv_set api"),
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

    pub fn exec(&self, query: SqlQuery) -> Result<sql::SqlResult, FxSqlError> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut sql_exec_request = op.init_sql_exec();
        sql_exec_request.set_database(&self.name);
        let mut request_query = sql_exec_request.init_query();
        request_query.set_statement(query.stmt);
        let mut params = request_query.init_params(query.params.len() as u32);
        for (param_index, param) in query.params.into_iter().enumerate() {
            let mut request_param = params.reborrow().get(param_index as u32).init_value();
            match param {
                SqlValue::Null => request_param.set_null(()),
                SqlValue::Integer(v) => request_param.set_integer(v),
                SqlValue::Real(v) => request_param.set_real(v),
                SqlValue::Text(v) => request_param.set_text(v),
                SqlValue::Blob(v) => request_param.set_blob(&v),
            }
        }

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::SqlExec(v) => {
                let sql_exec_response = v.unwrap();
                match sql_exec_response.get_response().which().unwrap() {
                    fx_capnp::sql_exec_response::response::Which::BindingNotFound(_) => Err(FxSqlError::BindingNotExists),
                    fx_capnp::sql_exec_response::response::Which::SqlError(v) => Err(FxSqlError::QueryFailed { reason: v.unwrap().get_description().unwrap().to_string().unwrap() }),
                    fx_capnp::sql_exec_response::response::Which::Rows(rows) => {
                        let rows = rows.unwrap();
                        Ok(sql::SqlResult::from(rows))
                    }
                }
            },
            _other => panic!("unexpected response from sql_exec api"),
        }
    }

    pub fn batch(&self, queries: Vec<SqlQuery>) -> Result<(), FxSqlError> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut sql_batch_request = op.init_sql_batch();
        sql_batch_request.set_database(&self.name);
        let mut request_queries = sql_batch_request.init_queries(queries.len() as u32);

        for (query_index, query) in queries.into_iter().enumerate() {
            let mut request_query = request_queries.reborrow().get(query_index as u32);
            request_query.set_statement(query.stmt);
            let mut request_query_params = request_query.init_params(query.params.len() as u32);

            for (param_index, param) in query.params.into_iter().enumerate() {
                let mut request_param = request_query_params.reborrow().get(param_index as u32).init_value();
                match param {
                    SqlValue::Null => request_param.set_null(()),
                    SqlValue::Integer(v) => request_param.set_integer(v),
                    SqlValue::Real(v) => request_param.set_real(v),
                    SqlValue::Text(v) => request_param.set_text(v),
                    SqlValue::Blob(v) => request_param.set_blob(&v),
                }
            }
        }

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::SqlBatch(v) => {
                let sql_batch_response = v.unwrap();
                match sql_batch_response.get_response().which().unwrap() {
                    fx_capnp::sql_batch_response::response::Which::BindingNotFound(_) => Err(FxSqlError::BindingNotExists),
                    fx_capnp::sql_batch_response::response::Which::SqlError(v) => Err(FxSqlError::QueryFailed { reason: v.unwrap().get_description().unwrap().to_string().unwrap() }),
                    fx_capnp::sql_batch_response::response::Which::Ok(_) => Ok(()),
                }
            },
            _other => panic!("unexpected response from sql_batch api"),
        }
    }
}

pub async fn sleep(duration: Duration) {
    let future_id = {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut sleep_request = op.init_sleep();
        sleep_request.set_millis(duration.as_millis() as u64);

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::Sleep(v) => {
                let sleep = v.unwrap();
                match sleep.get_response().which().unwrap() {
                    fx_capnp::sleep_response::response::Which::FutureId(v) => v,
                    fx_capnp::sleep_response::response::Which::SleepError(err) => panic!("failed to sleep: {err:?}"),
                }
            },
            _other => panic!("unexpected response from sleep api"),
        }
    };

    FxHostFuture::new(PoolIndex(future_id)).await.unwrap();
}

pub fn read_rpc_request<T: serde::de::DeserializeOwned>(addr: i64, len: i64) -> Result<T, FxError> {
    rmp_serde::from_slice(&read_memory_owned(addr, len))
        .map_err(|err| FxError::DeserializationError { reason: format!("failed to deserialize: {err:?}") })
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
        let mut message = capnp::message::Builder::new_default();
        message.init_root::<fx_capnp::fx_api_call::Builder>()
            .init_op()
            .init_time();

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        let timestamp = match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::Time(v) => {
                let time_response = v.unwrap();
                time_response.get_timestamp()
            },
            _other => panic!("unexpected response from time api"),
        };

        Self {
            millis_since_unix: timestamp as i64,
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
