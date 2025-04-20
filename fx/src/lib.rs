pub use {
    fx_core::{HttpRequest, HttpResponse},
    fx_macro::rpc,
};

use {
    std::sync::atomic::{AtomicBool, Ordering},
    lazy_static::lazy_static,
    crate::{sys::read_memory, logging::FxLoggingLayer},
};

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

    pub fn rpc<T: bincode::Encode, R: bincode::Decode<()>>(&self, service_id: impl Into<String>, function: impl Into<String>, arg: T) -> R {
        let service_id = service_id.into();
        let service_id = service_id.as_bytes();
        let function = function.into();
        let function = function.as_bytes();
        let arg = bincode::encode_to_vec(arg, bincode::config::standard()).unwrap();
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

        bincode::decode_from_slice(&ptr_and_len.read_owned(), bincode::config::standard()).unwrap().0
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

pub fn read_rpc_request<T: bincode::Decode<()>>(addr: i64, len: i64) -> T {
    bincode::decode_from_slice(read_memory(addr, len), bincode::config::standard()).unwrap().0
}

pub fn write_rpc_response<T: bincode::Encode>(response: T) {
    let response = bincode::encode_to_vec(response, bincode::config::standard()).unwrap();
    unsafe { sys::send_rpc_response(response.as_ptr() as i64, response.len() as i64) };
}
