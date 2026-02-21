pub use self::{
    resource::{
        ResourceId,
        FunctionResourceId,
        FunctionResource,
        SerializableResource,
        add_function_resource,
        replace_function_resource,
        serialize_function_resource,
        drop_function_resource,
        map_function_resource_ref,
        map_function_resource_ref_mut,
    },
    future::wrap_function_response_future,
};

pub(crate) use self::{
    logs::log,
    resource::{DeserializableHostResource, DeserializeHostResource, FutureHostResource, OwnedResourceId, HostUnitFuture, SerializeResource},
};

use {
    std::{task::Poll, io::Cursor},
    futures::FutureExt,
    fx_types::{capnp, abi::FuturePollResult},
    crate::{
        logging::{set_panic_hook, init_logger},
    },
};

mod future;
mod logs;
mod resource;

// exports:
/// returns fx_types::abi::FunctionPollResult
#[unsafe(no_mangle)]
pub extern "C" fn _fx_future_poll(future_resource_id: u64) -> i64 {
    use std::task::{Context, Waker};

    let future_resource_id = FunctionResourceId::new(future_resource_id);
    let future_poll_result = map_function_resource_ref_mut(&future_resource_id, |future_resource| {
        let future = match &mut *future_resource {
            FunctionResource::FunctionResponseFuture(v) => v,
            _other => panic!("resource is not future"),
        };

        let mut context = Context::from_waker(Waker::noop());
        future.poll_unpin(&mut context)
    });

    (match future_poll_result {
        Poll::Pending => FuturePollResult::Pending,
        Poll::Ready(resource) => {
            let resource = FunctionResource::from(resource);
            replace_function_resource(&future_resource_id, resource);
            FuturePollResult::Ready
        }
    }) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_resource_serialize(resource_id: u64) -> u64 {
    serialize_function_resource(&FunctionResourceId::new(resource_id))
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_resource_serialized_ptr(resource_id: u64) -> i64 {
    map_function_resource_ref(&FunctionResourceId::new(resource_id), |resource| {
        match &*resource {
            FunctionResource::FunctionResponseFuture(_) => panic!("not a serialized resource"),
            FunctionResource::FunctionResponse(v) => match v {
                SerializableResource::Raw(_) => panic!("resource has to be serialized first"),
                SerializableResource::Serialized(v) => v.as_ptr(),
            },
            FunctionResource::FunctionResponseBody(v) => v.as_ptr(),
        }
    }) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_resource_drop(resource_id: u64) {
    drop_function_resource(&FunctionResourceId::new(resource_id));
}

// imports:
#[link(wasm_import_module = "fx")]
unsafe extern "C" {
    pub(crate) fn fx_log(req_addr: i64, req_len: i64);
    pub(crate) fn fx_resource_serialize(resource_id: u64) -> u64;
    pub(crate) fn fx_resource_move_from_host(resource_id: u64, ptr: u64);
    pub(crate) fn fx_resource_drop(resource_id: u64);
    pub(crate) fn fx_sql_exec(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_sql_migrate(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_future_poll(future_resource_id: u64) -> i64;
    pub(crate) fn fx_sleep(sleep_millis: u64) -> u64;
    pub(crate) fn fx_random(ptr: u64, len: u64);
    pub(crate) fn fx_time() -> u64;
    pub(crate) fn fx_blob_put(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64, value_ptr: u64, value_len: u64) -> u64;
    pub(crate) fn fx_blob_get(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64) -> u64;
    pub(crate) fn fx_blob_delete(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64) -> u64;
    pub(crate) fn fx_fetch(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_metrics_counter_register(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_metrics_counter_increment(metric_id: u64, delta: u64);
    pub(crate) fn fx_stream_frame_read(resource_id: u64, ptr: u64);
}

#[derive(Debug)]
#[repr(C)]
pub struct PtrWithLen {
    pub ptr: i64,
    pub len: i64,
}

impl PtrWithLen {
    pub fn new() -> Self {
        Self {
            ptr: 0,
            len: 0,
        }
    }

    pub fn ptr_to_self(&self) -> i64 {
        self as *const PtrWithLen as i64
    }

    #[allow(dead_code)]
    pub fn read(&self) -> &[u8] {
        read_memory(self.ptr, self.len)
    }

    pub fn read_owned(&self) -> Vec<u8> {
        read_memory_owned(self.ptr, self.len)
    }

    pub fn read_decode<T: serde::de::DeserializeOwned>(&self) -> T {
        rmp_serde::from_slice(&self.read_owned()).unwrap()
    }
}

impl Default for PtrWithLen {
    fn default() -> Self {
        Self::new()
    }
}

// utils:
pub(crate) fn read_memory<'a>(ptr: i64, len: i64) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) }
}

pub(crate) fn read_memory_owned(ptr: i64, len: i64) -> Vec<u8> {
    unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) }
}
