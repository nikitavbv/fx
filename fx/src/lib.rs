pub use {
    std::sync::atomic::{AtomicBool, Ordering},
    lazy_static::lazy_static,
    fx_core::{HttpRequest, HttpResponse},
    fx_macro::handler,
    crate::logging::FxLoggingLayer,
};

use crate::sys::read_memory;

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

pub fn read_http_request(addr: i64, len: i64) -> HttpRequest {
    bincode::decode_from_slice(read_memory(addr, len), bincode::config::standard()).unwrap().0
}

pub fn send_http_response(response: HttpResponse) {
    let response = bincode::encode_to_vec(response, bincode::config::standard()).unwrap();
    unsafe { sys::send_http_response(response.as_ptr() as i64, response.len() as i64); }
}
