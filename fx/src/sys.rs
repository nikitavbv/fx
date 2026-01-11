use {
    std::{task::Poll, io::Cursor},
    fx_api::{capnp, fx_capnp},
    crate::{
        fx_futures::{FUTURE_POOL, PoolIndex},
        fx_streams::STREAM_POOL,
        logging::{set_panic_hook, init_logger},
        api::{handle_future_poll, handle_future_drop, handle_stream_drop, handle_stream_poll_next, handle_invoke},
    },
};

// exports:
// malloc is exported function because it saves roundrip for capnp-based api
#[unsafe(no_mangle)]
pub extern "C" fn _fx_malloc(size: i64) -> i64 {
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size as usize, 1).unwrap()) as i64 }
}

// dealloc is exported function to avoid RPC memory management depend on using capnp-based api
#[unsafe(no_mangle)]
pub extern "C" fn _fx_dealloc(ptr: i64, size: i64) {
    unsafe { std::alloc::dealloc(ptr as *mut u8, std::alloc::Layout::from_size_align(size as usize, 1).unwrap()) }
}

// main entrypoint for capnp-based api
#[unsafe(no_mangle)]
pub extern "C" fn _fx_api(req_addr: i64, req_len: i64) -> i64 {
    set_panic_hook();
    init_logger();

    let mut message_bytes = unsafe { Vec::from_raw_parts(req_addr as *mut u8, req_len as usize, req_len as usize) };
    let message_reader = capnp::serialize::read_message_from_flat_slice(&mut message_bytes.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
    let request = message_reader.get_root::<fx_api::fx_capnp::fx_function_api_call::Reader>().unwrap();
    let op = request.get_op();

    let mut response_message = capnp::message::Builder::new_default();
    let response = response_message.init_root::<fx_capnp::fx_function_api_call_result::Builder>();
    let mut response_op = response.init_op();

    use fx_capnp::fx_function_api_call::op::{Which as Operation};
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
            handle_invoke(v.unwrap(), response_op.init_invoke());
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
    pub(crate) fn send_error(ptr: i64, len: i64); // TODO: replace with unified api handler in fx sdk
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
