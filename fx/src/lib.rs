pub use {
    fx_core::{HttpRequest, HttpResponse, FetchRequest, FetchResponse, SqlQuery, DatabaseSqlQuery, SqlResult, SqlValue, CronRequest},
    fx_macro::rpc,
    futures::FutureExt,
    crate::{sys::PtrWithLen, fx_futures::FxFuture},
};

use {
    std::{sync::atomic::{AtomicBool, Ordering}, panic},
    lazy_static::lazy_static,
    crate::{sys::read_memory, logging::FxLoggingLayer, fx_futures::{FxHostFuture, PoolIndex}},
};

mod fx_futures;
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

    pub fn rpc<T: serde::ser::Serialize, R: serde::de::DeserializeOwned>(&self, service_id: impl Into<String>, function: impl Into<String>, arg: T) -> R {
        let service_id = service_id.into();
        let service_id = service_id.as_bytes();
        let function = function.into();
        let function = function.as_bytes();
        let arg = rmp_serde::to_vec(&arg).unwrap();
        let arg = arg.as_slice();

        let ptr_and_len = sys::PtrWithLen::new();

        unsafe {
            sys::rpc(
                service_id.as_ptr() as i64,
                service_id.len() as i64,
                function.as_ptr() as i64,
                function.len() as i64,
                arg.as_ptr() as i64,
                arg.len() as i64,
                ptr_and_len.ptr_to_self()
            );
        }

        rmp_serde::from_slice(&ptr_and_len.read_owned()).unwrap()
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

    pub fn fetch(&self, req: FetchRequest) -> FetchResponse {
        let req = rmp_serde::to_vec(&req).unwrap();
        let ptr_and_len = sys::PtrWithLen::new();
        unsafe { sys::fetch(req.as_ptr() as i64, req.len() as i64, ptr_and_len.ptr_to_self()); }

        rmp_serde::from_slice(&ptr_and_len.read_owned()).unwrap()
    }
}

pub struct KvStore {
    namespace: String,
}

impl KvStore {
    pub(crate) fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
        }
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let key = self.namespaced(key);
        let key = key.as_bytes();
        let ptr_and_len = sys::PtrWithLen::new();
        if unsafe { sys::kv_get(key.as_ptr() as i64, key.len() as i64, ptr_and_len.ptr_to_self()) } == 0 {
            Some(ptr_and_len.read_owned())
        } else {
            None
        }
    }

    pub fn set(&self, key: &str, value: &[u8]) {
        let key = self.namespaced(key);
        let key = key.as_bytes();
        unsafe { sys::kv_set(key.as_ptr() as i64, key.len() as i64, value.as_ptr() as i64, value.len() as i64) };
    }

    fn namespaced(&self, key: &str) -> String {
        format!("{}/{}", self.namespace, key)
    }
}

#[derive(Clone, Debug)]
pub struct SqlDatabase {
    name: String,
}

impl SqlDatabase {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    pub fn exec(&self, query: SqlQuery) -> SqlResult {
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

pub async fn sleep() {
    FxHostFuture::new(PoolIndex(unsafe { sys::sleep() } as u64)).await;
}

pub fn read_rpc_request<T: serde::de::DeserializeOwned>(addr: i64, len: i64) -> T {
    rmp_serde::from_slice(read_memory(addr, len)).unwrap()
}

pub fn write_rpc_response<T: serde::ser::Serialize>(response: T) {
    write_rpc_response_raw(rmp_serde::to_vec(&response).unwrap());
}

pub fn write_rpc_response_raw(response: Vec<u8>) {
    unsafe { sys::send_rpc_response(response.as_ptr() as i64, response.len() as i64) };
}

pub fn panic_hook(info: &panic::PanicHookInfo) {
    let payload = info.payload().downcast_ref::<&str>()
        .map(|v| v.to_owned().to_owned())
        .or(info.payload().downcast_ref::<String>().map(|v| v.to_owned()));
    tracing::error!("fx module panic: {info:?}, payload: {payload:?}");
}

pub fn to_vec<T: serde::ser::Serialize>(v: T) -> Vec<u8> {
    rmp_serde::to_vec(&v).unwrap()
}
