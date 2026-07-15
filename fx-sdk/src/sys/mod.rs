pub use self::{
    resource::{FunctionResourceId, FetchRequestHeaderResourceId},
    future::wrap_function_response_future,
};

pub(crate) use self::{
    logs::log,
    resource::{
        SerializableResource,
        HostUnitFuture,
        FunctionResource,
        BytesResource,
        add_function_resource,
        replace_function_resource,
        serialize_function_resource,
        drop_function_resource,
        map_function_resource_ref,
        map_function_resource_ref_mut,
        replace_function_resource_with_effect,
    },
};

use {
    std::task::Poll,
    futures::{FutureExt, StreamExt},
    fx_types::{capnp, abi::FuturePollResult, abi_http_capnp},
    crate::{
        api::http::{HttpBody, HttpBodyInner, serialize_http_body_full},
    },
    self::resource::replace_function_resource_with,
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
        let mut context = Context::from_waker(Waker::noop());

        match &mut *future_resource {
            FunctionResource::FunctionResponseFuture(v) => v
                .poll_unpin(&mut context)
                .map(|v| Some(FunctionResource::from(v))),
            FunctionResource::BackgroundTask(v) => match v.poll_unpin(&mut context) {
                std::task::Poll::Pending => std::task::Poll::Pending,
                std::task::Poll::Ready(_) => std::task::Poll::Ready(None),
            },
            _other => panic!("resource is not future"),
        }
    });

    (match future_poll_result {
        Poll::Pending => FuturePollResult::Pending,
        Poll::Ready(None) => FuturePollResult::Ready,
        Poll::Ready(Some(resource)) => {
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
            FunctionResource::HttpBody(body) => match body.0 {
                HttpBodyInner::Bytes(_) => panic!("resource has to be serialized first"),
                HttpBodyInner::HostResource { .. } => panic!("resource of this type should not be serialized: instead host should read it directly from host resource table"),
                HttpBodyInner::Empty => panic!("empty body: nothing to serailize"),
                HttpBodyInner::Stream { stream: _, ref frame_serialized } => frame_serialized.as_ref().unwrap().as_ptr(),
                HttpBodyInner::Serialized(ref v) => v.as_ptr(),
            },
            FunctionResource::BackgroundTask(_) => panic!("resource of this type cannot be serialized"),
        }
    }) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_resource_drop(resource_id: u64) {
    drop_function_resource(&FunctionResourceId::new(resource_id));
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_stream_frame_poll(resource_id: u64) -> i64 {
    use std::task::{Context, Waker};
    let mut context = Context::from_waker(Waker::noop());

    let poll_result = replace_function_resource_with_effect(FunctionResourceId::new(resource_id), |resource| {
        match resource {
            FunctionResource::HttpBody(v) => match v.0 {
                HttpBodyInner::Stream { mut stream, frame_serialized: _previous_frame_discarded } =>
                    match stream.poll_next_unpin(&mut context) {
                        Poll::Pending => (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Stream { stream, frame_serialized: None })), Poll::Pending),
                        Poll::Ready(v) => (
                            FunctionResource::HttpBody(HttpBody(HttpBodyInner::Stream {
                                stream,
                                frame_serialized: Some({
                                    let mut message = capnp::message::Builder::new_default();
                                    let serialized_frame = message.init_root::<abi_http_capnp::http_body_frame::Builder>();
                                    let mut serialized_frame = serialized_frame.init_frame();

                                    match v {
                                        None => serialized_frame.set_stream_end(()),
                                        Some(Err(err)) => todo!("handle error: {err:?}"),
                                        Some(Ok(frame)) => serialized_frame.set_bytes(&frame.to_vec()),
                                    }

                                    capnp::serialize::write_message_to_words(&message)
                                }),
                            })),
                            Poll::Ready(())
                        ),
                    },
                HttpBodyInner::Empty => (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Empty)), Poll::Ready(())),
                HttpBodyInner::Bytes(v) => (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Bytes(v))), Poll::Ready(())),
                HttpBodyInner::HostResource { .. } => panic!("stream poll should not be called for host resources"),
                HttpBodyInner::Serialized(_) => panic!("stream poll called for non-stream body!"),
            },
            _other => panic!("not a stream"),
        }
    });

    (match poll_result {
        Poll::Pending => FuturePollResult::Pending,
        Poll::Ready(()) => FuturePollResult::Ready,
    }) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_stream_frame_serialize(resource_id: u64) -> u64 {
    replace_function_resource_with_effect(FunctionResourceId::new(resource_id), |resource| {
        match resource {
            FunctionResource::HttpBody(v) => match v.0 {
                HttpBodyInner::Empty => {
                    let mut message = capnp::message::Builder::new_default();
                    let serialized_frame = message.init_root::<abi_http_capnp::http_body_frame::Builder>();
                    let mut serialized_frame = serialized_frame.init_frame();

                    serialized_frame.set_stream_end(());

                    let serialized_frame = capnp::serialize::write_message_to_words(&message);
                    let serialized_len = serialized_frame.len();

                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(serialized_frame))), serialized_len)
                },
                HttpBodyInner::Bytes(v) => {
                    let serialized = serialize_http_body_full(v);
                    let serialized_size = serialized.len();
                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(serialized))), serialized_size)
                },
                HttpBodyInner::Stream { stream, frame_serialized } => {
                    let serialized_size = frame_serialized.as_ref().unwrap().len();
                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Stream { stream, frame_serialized })), serialized_size)
                },
                HttpBodyInner::HostResource { .. } => panic!("stream_frame_serialized cannot be invoked on HttpBody of this type, resource_id: {resource_id:?}"),
                HttpBodyInner::Serialized(v) => {
                    let serialized_len = v.len();
                    (FunctionResource::HttpBody(HttpBody(HttpBodyInner::Serialized(v))), serialized_len)
                },
            },
            _other => panic!("stream_frame_serialized cannot be invoked on FunctionResource of this type"),
        }
    }) as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_stream_advance(resource_id: u64) {
    replace_function_resource_with(FunctionResourceId::new(resource_id), |v| match v {
        FunctionResource::FunctionResponse(_) => panic!("not a stream"),
        FunctionResource::FunctionResponseFuture(_) => panic!("not a stream"),
        FunctionResource::BackgroundTask(_) => panic!("not a stream"),
        FunctionResource::HttpBody(v) => match v.0 {
            HttpBodyInner::Empty
            | HttpBodyInner::HostResource { .. } => todo!(),
            HttpBodyInner::Bytes(_) => todo!(),
            HttpBodyInner::Stream { stream, frame_serialized } => FunctionResource::HttpBody(HttpBody(HttpBodyInner::Stream {
                stream,
                frame_serialized,
            })),
            HttpBodyInner::Serialized(_) => FunctionResource::HttpBody(HttpBody(HttpBodyInner::Empty)),
        },
    });
}

// imports:
#[link(wasm_import_module = "fx")]
unsafe extern "C" {
    pub(crate) fn fx_log(req_addr: i64, req_len: i64);
    pub(crate) fn fx_sql_exec(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_sql_batch(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_sql_migrate(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_sleep(sleep_millis: u64) -> u64;
    pub(crate) fn fx_random(ptr: u64, len: u64);
    pub(crate) fn fx_time() -> u64;
    pub(crate) fn fx_blob_put(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64, value_ptr: u64, value_len: u64) -> u64;
    pub(crate) fn fx_blob_get(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64) -> u64;
    pub(crate) fn fx_blob_delete(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64) -> u64;
    pub(crate) fn fx_fetch(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_metrics_counter_register(req_addr: u64, req_len: u64) -> u64;
    pub(crate) fn fx_metrics_counter_increment(metric_id: u64, delta: u64);
    pub(crate) fn fx_env_len(key_ptr: u64, key_len: u64) -> i64;
    pub(crate) fn fx_env_get(key_ptr: u64, key_len: u64, value_ptr: u64);
    pub(crate) fn fx_kv_set(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64, value_ptr: u64, value_len: u64) -> u64;
    pub(crate) fn fx_kv_set_nx_px(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64, value_ptr: u64, value_len: u64, nx: u32, px: i64) -> u64;
    pub(crate) fn fx_kv_get(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64) -> u64;
    pub(crate) fn fx_kv_delex_ifeq(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64, ifeq_ptr: u64, ifeq_len: u64) -> u64;
    pub(crate) fn fx_kv_subscribe(binding_ptr: u64, binding_len: u64, channel_addr: u64, channel_len: u64) -> u64;
    pub(crate) fn fx_kv_publish(binding_ptr: u64, binding_len: u64, channel_addr: u64, channel_len: u64, data_addr: u64, data_len: u64) -> u64;
    pub(crate) fn fx_tasks_background_spawn(function_resource_id: u64);
    pub(crate) fn fx_fetch_request_header_serialize(resource_id: u64) -> u64;
    pub(crate) fn fx_bytes_len(resource_id: u64) -> u64;
    pub(crate) fn fx_bytes_move(resource_id: u64, ptr: u64) -> u64;
    pub(crate) fn fx_kv_get_response_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_kv_get_response_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_kv_set_response_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_kv_set_response_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_kv_subscription_stream_poll_next(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_unit_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_sql_query_result_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_sql_query_result_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_sql_batch_result_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_sql_batch_result_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_migration_result_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_migration_result_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_fetch_result_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_fetch_result_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_http_body_poll_frame(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_http_frame_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_get_result_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_get_result_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_delete_result_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_delete_result_serailize(resource_id: u64, result_addr: u64) -> u64;
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
