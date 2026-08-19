pub use self::{
    resource::{FunctionResourceId, FetchRequestHeaderResourceId, FunctionResponseFutureResourceKey},
    future::wrap_function_response_future,
};

pub(crate) use self::{
    logs::log,
    resource::{
        RESOURCE_SET,
        HostUnitFuture,
        BytesResource,
    },
};

use {
    std::task::Poll,
    tracing::error,
    futures::{FutureExt, StreamExt},
    fx_types::{capnp, abi::{FunctionBytesPtrAndLenResult, FunctionHttpBodyFramePollResult, FunctionResponsePollResult}, abi_http_capnp},
    crate::{
        api::http::HttpBodyInner,
        io::http::HttpStreamError,
        handler_fn::{FunctionResponseInner, FunctionHttpResponseBody},
    },
};

mod future;
mod logs;
pub(crate) mod resource;

// exports:
static mut BYTES_PTR_AND_LEN_RESULT: FunctionBytesPtrAndLenResult = FunctionBytesPtrAndLenResult {
    ptr: 0,
    len: 0,
};

static mut HTTP_BODY_FRAME_POLL_RESULT: FunctionHttpBodyFramePollResult = FunctionHttpBodyFramePollResult {
    tag: 0,
    _pad: [0; 7],
    frame_bytes_resource_id: 0,
    frame_bytes_addr: 0,
    frame_bytes_len: 0,
};

static mut FUNCTION_RESPONSE_POLL_RESULT: FunctionResponsePollResult = FunctionResponsePollResult {
    tag: 0,
    _pad: [0; 7],
    response_bytes_resource_id: 0,
    response_bytes_addr: 0,
    response_bytes_len: 0,
};

#[unsafe(no_mangle)]
pub extern "C" fn _fx_http_body_frame_poll(resource_id: u64) -> u64 {
    use std::task::{Context, Waker};
    let mut context = Context::from_waker(Waker::noop());

    RESOURCE_SET.with_borrow_mut(|resources| {
        let result = match &mut resources.http_bodies.get_mut(resource_id.into()).as_mut().unwrap().0 {
            HttpBodyInner::Stream(stream) => {
                match stream.poll_next_unpin(&mut context) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(frame) => {
                        Poll::Ready(frame.map(|v| v.map(|v| v.to_vec())))
                    }
                }
            },
            HttpBodyInner::Empty => Poll::Ready(None),
            HttpBodyInner::HostResource(_) => panic!("stream poll should not be called for host resources"), // TODO: http body should be split into seperate resources
        };

        let result = match result {
            Poll::Pending => FunctionHttpBodyFramePollResult {
                tag: 2,
                ..Default::default()
            },
            Poll::Ready(None) => FunctionHttpBodyFramePollResult {
                tag: 0,
                ..Default::default()
            },
            Poll::Ready(Some(Ok(v))) => FunctionHttpBodyFramePollResult {
                tag: 1,
                _pad: Default::default(),
                frame_bytes_addr: v.as_ptr() as u64,
                frame_bytes_len: v.len() as u64,
                frame_bytes_resource_id: resources.bytes.insert(v).into(),
            },
            Poll::Ready(Some(Err(HttpStreamError::AxumStreamRead(err)))) => {
                error!("axum stream error when reading http body: {err:?}");
                FunctionHttpBodyFramePollResult {
                    tag: 3,
                    ..Default::default()
                }
            },
        };

        unsafe {
            std::ptr::addr_of_mut!(HTTP_BODY_FRAME_POLL_RESULT).write(result);
        }

        std::ptr::addr_of!(HTTP_BODY_FRAME_POLL_RESULT) as u64
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_bytes_ptr_and_len(resource_id: u64) -> u64 {
    let (ptr, len) = RESOURCE_SET.with_borrow_mut(|resources| {
        let bytes = resources.bytes.get(resource_id.into()).unwrap();
        (bytes.as_ptr() as u64, bytes.len() as u64)
    });

    unsafe { std::ptr::addr_of_mut!(BYTES_PTR_AND_LEN_RESULT).write(FunctionBytesPtrAndLenResult { ptr, len }); }
    std::ptr::addr_of!(BYTES_PTR_AND_LEN_RESULT) as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_bytes_drop(resource_id: u64) {
    RESOURCE_SET.with_borrow_mut(|resources| {
        resources.bytes.remove(resource_id.into());
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_background_task_poll(resource_id: u64) -> u64 {
    use std::task::{Context, Waker};
    let mut context = Context::from_waker(Waker::noop());

    let mut task_future = RESOURCE_SET.with_borrow_mut(|resources| resources.background_tasks.detach(resource_id.into())).unwrap();

    let result = task_future.poll_unpin(&mut context);

    RESOURCE_SET.with_borrow_mut(|resources| resources.background_tasks.reattach(resource_id.into(), task_future));

    match result {
        Poll::Pending => 1,
        Poll::Ready(()) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_function_response_poll(resource_id: u64) -> u64 {
    use std::task::{Context, Waker};
    let mut context = Context::from_waker(Waker::noop());

    let mut response_future = RESOURCE_SET.with_borrow_mut(|resources| resources.function_response_futures.detach(resource_id.into())).unwrap();
    let result = response_future.poll_unpin(&mut context);

    let result = match result {
        Poll::Pending => FunctionResponsePollResult {
            tag: 1,
            ..Default::default()
        },
        Poll::Ready(response) => {
            let mut message = capnp::message::Builder::new_default();
            let mut resource = message.init_root::<abi_http_capnp::http_response::Builder>();
            match response.0 {
                FunctionResponseInner::HttpResponse(http) => {
                    resource.set_status(http.status.as_u16());

                    let mut headers = resource.reborrow().init_headers(http.headers.len() as u32);
                    for (index, (name, value)) in http.headers.iter().enumerate() {
                        let mut header = headers.reborrow().get(index as u32);
                        header.set_name(name.as_str());
                        header.set_value(value.to_str().unwrap());
                    }

                    let mut body = resource.init_body();
                    match http.body {
                        FunctionHttpResponseBody::FunctionResource(v) => body.set_function_resource_id(v.into()),
                        FunctionHttpResponseBody::HostResource(v) => body.set_host_resource_id(v),
                    }
                }
            }
            let response = capnp::serialize::write_message_to_words(&message);

            FunctionResponsePollResult {
                tag: 0,
                _pad: Default::default(),
                response_bytes_addr: response.as_ptr() as u64,
                response_bytes_len: response.len() as u64,
                response_bytes_resource_id: RESOURCE_SET.with_borrow_mut(|resources| resources.bytes.insert(response)).into(),
            }
        }
    };

    RESOURCE_SET.with_borrow_mut(|resources| resources.function_response_futures.reattach(resource_id.into(), response_future));

    unsafe {
        std::ptr::addr_of_mut!(FUNCTION_RESPONSE_POLL_RESULT).write(result);
    }

    std::ptr::addr_of!(FUNCTION_RESPONSE_POLL_RESULT) as u64
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
    pub(crate) fn fx_metrics_counter_register(req_addr: u64, req_len: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_metrics_counter_increment(metric_id: u64, delta: u64);
    pub(crate) fn fx_env_len(key_ptr: u64, key_len: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_env_get(key_ptr: u64, key_len: u64, value_ptr: u64) -> u64;
    pub(crate) fn fx_kv_delex_ifeq(binding_ptr: u64, binding_len: u64, key_ptr: u64, key_len: u64, ifeq_ptr: u64, ifeq_len: u64) -> u64;
    pub(crate) fn fx_kv_subscribe(binding_ptr: u64, binding_len: u64, channel_addr: u64, channel_len: u64) -> u64;
    pub(crate) fn fx_kv_publish(binding_ptr: u64, binding_len: u64, channel_addr: u64, channel_len: u64, data_addr: u64, data_len: u64) -> u64;
    pub(crate) fn fx_tasks_background_spawn(function_resource_id: u64);
    pub(crate) fn fx_fetch_request_header_serialize(resource_id: u64) -> u64;
    pub(crate) fn fx_bytes_len(resource_id: u64) -> u64;
    pub(crate) fn fx_bytes_move(resource_id: u64, ptr: u64) -> u64;
    pub(crate) fn fx_kv_delex_result_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_kv_delex_result_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_kv_subscription_stream_poll_next(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_kv_publish_result_future_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_kv_publish_result_serialize(resource_id: u64, result_addr: u64) -> u64;
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
    pub(crate) fn fx_blob_put_result_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_put_result_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_get_result_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_get_result_serialize(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_delete_result_poll(resource_id: u64, result_addr: u64) -> u64;
    pub(crate) fn fx_blob_delete_result_serialize(resource_id: u64, result_addr: u64) -> u64;
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
