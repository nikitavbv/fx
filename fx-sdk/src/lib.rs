pub use {
    fx_common::{SqlQuery, FxSqlError, SqlValue},
    futures::FutureExt,
    inventory,
    ::http::StatusCode,
    crate::{
        sys::PtrWithLen,
        error::FxError,
        FxResult as Result,
        api::{http::{HttpRequest, HttpResponse}, blob::blob, metrics},
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
            fx_sql_exec,
            fx_sleep,
            HostUnitFuture,
            fx_random,
            fx_time,
        },
        sql::{SqlResult, SqlError},
        logging::FxLoggingLayer,
    },
};

pub mod io {
    pub use crate::api::{blob, http};
}

pub mod sys;
pub mod utils;

pub mod handler;
pub mod logging;
pub mod sql;

mod api;
mod error;

pub type FxResult<T> = anyhow::Result<T>;

pub fn random(len: u64) -> Vec<u8> {
    let random_data = vec![0; len as usize];
    unsafe { fx_random(random_data.as_ptr() as u64, len); }
    random_data
}

pub fn now() -> FxInstant {
    FxInstant::now()
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

    pub async fn exec(&self, query: SqlQuery) -> StdResult<sql::SqlResult, SqlError> {
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
        let resource: FutureHostResource<StdResult<SqlResult, SqlError>> = FutureHostResource::new(resource_id);

        resource.await
    }

    /*pub fn batch(&self, queries: Vec<SqlQuery>) -> StdResult<(), FxSqlError> {
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
    }*/
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
