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
    resource::{DeserializableHostResource, DeserializeHostResource, FutureHostResource, OwnedResourceId, HostUnitFuture},
};

use {
    std::{task::Poll, io::Cursor},
    futures::FutureExt,
    fx_types::{capnp, abi_capnp, abi::FuturePollResult},
    crate::{
        fx_futures::{FUTURE_POOL, PoolIndex},
        fx_streams::STREAM_POOL,
        logging::{set_panic_hook, init_logger},
        api::{handle_future_poll, handle_future_drop, handle_stream_drop, handle_stream_poll_next},
    },
};

mod future;
mod logs;
mod resource;

// exports:
#[unsafe(no_mangle)]
pub extern "C" fn _fx_malloc(size: i64) -> i64 {
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size as usize, 1).unwrap()) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_dealloc(ptr: i64, size: i64) {
    unsafe { std::alloc::dealloc(ptr as *mut u8, std::alloc::Layout::from_size_align(size as usize, 1).unwrap()) }
}

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

// main entrypoint for capnp-based api (global dispatch will be deprecated soon)
#[unsafe(no_mangle)]
pub extern "C" fn _fx_api(req_addr: i64, req_len: i64) -> i64 {
    set_panic_hook();
    init_logger();

    let mut message_bytes = unsafe { Vec::from_raw_parts(req_addr as *mut u8, req_len as usize, req_len as usize) };
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
    let request = message_reader.get_root::<fx_types::abi_capnp::fx_function_api_call::Reader>().unwrap();
    let op = request.get_op();

    let mut response_message = capnp::message::Builder::new_default();
    let response = response_message.init_root::<abi_capnp::fx_function_api_call_result::Builder>();
    let mut response_op = response.init_op();

    use abi_capnp::fx_function_api_call::op::{Which as Operation};
    match op.which().unwrap() {
        Operation::FuturePoll(v) => {
            handle_future_poll(v.unwrap(), response_op.init_future_poll());
        },
        Operation::FutureDrop(v) => {
            handle_future_drop(v.unwrap(), response_op.init_future_drop());
        },
        Operation::StreamDrop(v) => {
            handle_stream_drop(v.unwrap(), response_op.init_stream_drop());
        },
        Operation::StreamPollNext(v) => {
            handle_stream_poll_next(v.unwrap(), response_op.init_stream_poll_next());
        },
        Operation::Invoke(v) => {
            unimplemented!("legacy invoke api is not supported anymore")
        }
    };

    let response_size = capnp::serialize::compute_serialized_size_in_words(&response_message) * 8;
    let response_size_bytes = response_size.to_le_bytes();

    let response_header_prefix_size = response_size_bytes.len(); // add header in the front to store response length
    assert!(response_header_prefix_size == 4);

    let ptr = unsafe { std::alloc::alloc(
        std::alloc::Layout::from_size_align(response_size as usize + response_header_prefix_size, 1).unwrap()
    ) };
    let mut response_slice = unsafe { std::slice::from_raw_parts_mut(ptr, response_size + response_header_prefix_size) };
    assert!(response_slice.len() == response_size + response_header_prefix_size);

    unsafe {
        std::ptr::copy_nonoverlapping(response_size_bytes.as_ptr(), ptr, response_header_prefix_size);
    }
    capnp::serialize::write_message(&mut response_slice[response_header_prefix_size..], &response_message).unwrap();

    ptr as i64
}

// imports:
#[link(wasm_import_module = "fx")]
unsafe extern "C" {
    pub(crate) fn fx_api(req_addr: i64, req_len: i64, output_ptr: i64);
    pub(crate) fn fx_log(req_addr: i64, req_len: i64);
    pub(crate) fn fx_resource_serialize(resource_id: u64) -> u64;
    pub(crate) fn fx_resource_move_from_host(resource_id: u64, ptr: u64);
    pub(crate) fn fx_resource_drop(resource_id: u64);
    pub(crate) fn fx_sql_exec(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_future_poll(future_resource_id: u64) -> i64;
    pub(crate) fn fx_sleep(sleep_millis: u64) -> u64;
    pub(crate) fn fx_random(ptr: u64, len: u64);
    pub(crate) fn fx_time() -> u64;
    pub(crate) fn fx_blob_put(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64, value_ptr: u64, value_len: u64) -> u64;
    pub(crate) fn fx_blob_get(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64) -> u64;
    pub(crate) fn fx_blob_delete(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64) -> u64;
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

pub(crate) fn invoke_fx_api(message: capnp::message::Builder<capnp::message::HeapAllocator>) -> capnp::message::Reader<capnp::serialize::OwnedSegments> {
    let message = capnp::serialize::write_message_to_words(&message);
    let output_ptr = PtrWithLen::default();
    unsafe { fx_api(message.as_ptr() as i64, message.len() as i64, output_ptr.ptr_to_self()) };
    let response = output_ptr.read_owned();
    capnp::serialize::read_message(Cursor::new(response), capnp::message::ReaderOptions::default()).unwrap()
}
