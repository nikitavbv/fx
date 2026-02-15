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
        HeaderValue,
    },
    fx_macro::handler,
    futures::FutureExt,
    inventory,
    ::http::StatusCode,
    crate::{
        sys::PtrWithLen,
        fx_futures::FxFuture,
        fx_streams::{FxStream, FxStreamExport, FxStreamImport},
        error::FxError,
        http::{FxHttpRequest},
        handler::{Handler, IntoHandler},
        api::{HttpRequestV2, BlobBucket, blob, BlobGetError, fetch, metrics},
        FxResult as Result,
    },
};

use {
    std::{sync::Once, panic, time::Duration, ops::Sub, result::Result as StdResult},
    lazy_static::lazy_static,
    thiserror::Error,
    chrono::{DateTime, Utc, TimeZone},
    fx_types::{capnp, abi_capnp, abi_sql_capnp},
    crate::{
        sys::{
            OwnedResourceId,
            FutureHostResource,
            invoke_fx_api,
            fx_sql_exec,
            fx_sleep,
            HostUnitFuture,
            fx_random,
            fx_time,
        },
        sql::SqlResult,
        logging::FxLoggingLayer,
        fx_futures::{FxHostFuture, PoolIndex, HostFutureError, HostFuturePollRuntimeError, HostFutureAsyncApiError},
    },
};

pub mod sys;
pub mod utils;

pub mod handler;
pub mod logging;
pub mod sql;

mod api;
mod error;
mod fx_futures;
mod fx_streams;
mod http;

pub type FxResult<T> = anyhow::Result<T>;

pub fn random(len: u64) -> Vec<u8> {
    let random_data = vec![0; len as usize];
    unsafe { fx_random(random_data.as_ptr() as u64, len); }
    random_data
}

pub fn now() -> FxInstant {
    FxInstant::now()
}

pub fn kv(namespace: impl Into<String>) -> KvStore {
    KvStore::new(namespace)
}

#[derive(Error, Debug)]
pub enum RpcError {
    /// rpc error failed because of error in runtime implementation
    /// Should never happen. If you see this error it means there is a bug somewhere.
    #[error("error in runtime implementation: {0:?}")]
    RuntimeError(RpcRuntimeError),

    /// Function being invoked returned an error
    #[error("received application error when invoked target function: {message:?}")]
    UserApplicationError {
        message: String,
    },

    /// Function being invoked panicked
    #[error("target function panicked")]
    FunctionPanicked,

    /// Handler not found within the function being invoked
    #[error("target function does not contain handler with this name")]
    HandlerNotFound,
}

#[derive(Error, Debug)]
pub enum RpcRuntimeError {
    #[error("failed to poll future: {0:?}")]
    FutureError(HostFuturePollRuntimeError),

    #[error("received unexpected async api response")]
    UnexpectedAsyncApiError,

    #[error("runtime error in rpc implementation on host side")]
    RpcRuntimeError,

    #[error("runtime error in rpc implementation on target function side (or it is not behaving properly)")]
    FunctionRuntimeError,
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

    pub fn get(&self, key: &str) -> StdResult<Option<Vec<u8>>, KvError> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<abi_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut kv_get_request = op.init_kv_get();
        kv_get_request.set_key(key.as_bytes());
        kv_get_request.set_binding_id(self.binding.as_str());
        let response = invoke_fx_api(message);
        let response = response.get_root::<abi_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            abi_capnp::fx_api_call_result::op::Which::KvGet(v) => {
                let kv_get_response = v.unwrap();
                match kv_get_response.get_response().which().unwrap() {
                    abi_capnp::kv_get_response::response::Which::BindingNotFound(_) => Err(KvError::BindingDoesNotExist),
                    abi_capnp::kv_get_response::response::Which::KeyNotFound(_) => Ok(None),
                    abi_capnp::kv_get_response::response::Which::Value(v) => Ok(Some(v.unwrap().to_vec())),
                }
            },
            _other => panic!("unexpected response from kv_get api"),
        }
    }

    pub fn set(&self, key: &str, value: &[u8]) -> StdResult<(), KvError> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<abi_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut kv_set_request = op.init_kv_set();
        kv_set_request.set_binding_id(self.binding.as_str());
        kv_set_request.set_key(key.as_bytes());
        kv_set_request.set_value(value);
        let response = invoke_fx_api(message);
        let response = response.get_root::<abi_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            abi_capnp::fx_api_call_result::op::Which::KvSet(v) => {
                let kv_set_response = v.unwrap();
                match kv_set_response.get_response().which().unwrap() {
                    abi_capnp::kv_set_response::response::Which::BindingNotFound(_) => Err(KvError::BindingDoesNotExist),
                    abi_capnp::kv_set_response::response::Which::Ok(_) => Ok(()),
                }
            },
            _other => panic!("unexpected response from kv_set api"),
        }
    }
}

pub fn sql(name: impl Into<String>) -> SqlDatabase {
    SqlDatabase::new(name.into())
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

    pub async fn exec(&self, query: SqlQuery) -> StdResult<sql::SqlResult, FxSqlError> {
        let message = {
            let mut message = capnp::message::Builder::new_default();
            let mut request = message.init_root::<abi_sql_capnp::sql_exec_request::Builder>();

            request.set_binding(&self.name);
            request.set_statement(query.stmt);

            let mut params = request.init_params(query.params.len() as u32);
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

            capnp::serialize::write_message_segments_to_words(&message)
        };

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_sql_exec(message.as_ptr() as u64, message.len() as u64) });
        let resource: FutureHostResource<SqlResult> = FutureHostResource::new(resource_id);

        Ok(resource.await)
    }

    pub fn batch(&self, queries: Vec<SqlQuery>) -> StdResult<(), FxSqlError> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<abi_capnp::fx_api_call::Builder>();
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
        let response = response.get_root::<abi_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            abi_capnp::fx_api_call_result::op::Which::SqlBatch(v) => {
                let sql_batch_response = v.unwrap();
                match sql_batch_response.get_response().which().unwrap() {
                    abi_capnp::sql_batch_response::response::Which::BindingNotFound(_) => Err(FxSqlError::BindingNotExists),
                    abi_capnp::sql_batch_response::response::Which::SqlError(v) => Err(FxSqlError::QueryFailed { reason: v.unwrap().get_description().unwrap().to_string().unwrap() }),
                    abi_capnp::sql_batch_response::response::Which::Ok(_) => Ok(()),
                }
            },
            _other => panic!("unexpected response from sql_batch api"),
        }
    }
}

pub async fn sleep(duration: Duration) {
    HostUnitFuture::new(OwnedResourceId::from_ffi(unsafe { fx_sleep(duration.as_millis() as u64) })).await
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
            millis_since_unix: unsafe { fx_time() } as i64,
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
