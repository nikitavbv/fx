pub use {
    futures::FutureExt,
    inventory,
    ::http::{StatusCode, HeaderName, HeaderValue},
    fx_macro::handler,
    crate::{
        sys::PtrWithLen,
        error::FxError,
        FxResult as Result,
        api::{http::{HttpRequest, HttpResponse}, blob::blob, metrics},
        sql::{SqlQuery, SqlError, SqlValue},
    },
};

use {
    std::{sync::Once, panic, time::Duration, ops::Sub, result::Result as StdResult},
    lazy_static::lazy_static,
    thiserror::Error,
    chrono::{DateTime, Utc, TimeZone},
    fx_types::{capnp, abi_sql_capnp},
    crate::{
        sys::{
            OwnedResourceId,
            FutureHostResource,
            fx_sql_exec,
            fx_sql_batch,
            fx_sleep,
            HostUnitFuture,
            fx_random,
            fx_time,
        },
        sql::{SqlResult, SqlBatchError},
        logging::FxLoggingLayer,
    },
};

pub mod io {
    pub use crate::api::{blob, http, kv, env};
}

pub mod sys;
pub mod utils;

pub mod handler_fn;
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

    pub async fn batch(&self, queries: Vec<SqlQuery>) -> StdResult<(), SqlBatchError> {
        let message = {
            let mut message = capnp::message::Builder::new_default();
            let mut request = message.init_root::<abi_sql_capnp::sql_batch_request::Builder>();

            request.set_binding(&self.name);
            let mut queries_builder = request.init_queries(queries.len() as u32);

            for (query_index, query) in queries.into_iter().enumerate() {
                let mut query_builder = queries_builder.reborrow().get(query_index as u32);
                query_builder.set_statement(&query.stmt);

                let mut params_builder = query_builder.init_params(query.params.len() as u32);
                for (param_index, param) in query.params.into_iter().enumerate() {
                    let mut request_param = params_builder.reborrow().get(param_index as u32).init_value();
                    match param {
                        SqlValue::Null => request_param.set_null(()),
                        SqlValue::Integer(v) => request_param.set_integer(v),
                        SqlValue::Real(v) => request_param.set_real(v),
                        SqlValue::Text(v) => request_param.set_text(v),
                        SqlValue::Blob(v) => request_param.set_blob(&v),
                    }
                }
            }

            capnp::serialize::write_message_segments_to_words(&message)
        };

        let resource_id = OwnedResourceId::from_ffi(unsafe { fx_sql_batch(message.as_ptr() as u64, message.len() as u64) });
        let resource: FutureHostResource<StdResult<(), SqlBatchError>> = FutureHostResource::new(resource_id);

        resource.await
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
