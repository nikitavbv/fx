use {
    std::task::Poll,
    crate::{
        fx_futures::{FUTURE_POOL, PoolIndex},
        fx_streams::STREAM_POOL,
        write_rpc_response_raw,
        set_panic_hook,
    },
};

// exports:
#[unsafe(no_mangle)]
pub extern "C" fn _fx_malloc(size: i64) -> i64 {
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size as usize, 1).unwrap()) as i64 }
}

/* returns 0 if pending, 1 if ready */
#[unsafe(no_mangle)]
pub extern "C" fn _fx_future_poll(future_index: i64) -> i64 {
    set_panic_hook();
    match FUTURE_POOL.poll(PoolIndex(future_index as u64)) {
        Poll::Pending => 0,
        Poll::Ready(v) => {
            write_rpc_response_raw(v);
            1
        }
    }
}

/* returns 0 if pending, 1 if ready (some), 2 if ready (none) */
#[unsafe(no_mangle)]
pub extern "C" fn _fx_stream_next(stream_index: i64) -> i64 {
    set_panic_hook();
    match STREAM_POOL.next(stream_index) {
        Poll::Pending => 0,
        Poll::Ready(Some(v)) => {
            write_rpc_response_raw(v);
            1
        },
        Poll::Ready(None) => 2,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _fx_stream_drop(stream_index: i64) {
    STREAM_POOL.remove(stream_index as u64);
}

// imports:
#[link(wasm_import_module = "fx")]
unsafe extern "C" {
    pub(crate) fn rpc(
        service_name_ptr: i64,
        service_name_len: i64,
        function_name_ptr: i64,
        function_name_len: i64,
        arg_ptr: i64,
        arg_len: i64,
    ) -> i64;
    pub(crate) fn rpc_async(
        service_name_ptr: i64,
        service_name_len: i64,
        function_name_ptr: i64,
        function_name_len: i64,
        arg_ptr: i64,
        arg_len: i64,
    );
    pub(crate) fn send_rpc_response(ptr: i64, len: i64);
    pub(crate) fn send_error(ptr: i64, len: i64);
    pub(crate) fn kv_get(binding_ptr: i64, binding_len: i64, k_ptr: i64, k_len: i64, output_ptr: i64) -> i64; // 0 - ok, 1 - binding does not exist, 2 - value does not exist
    pub(crate) fn kv_set(binding_ptr: i64, binding_len: i64, k_ptr: i64, k_len: i64, v_ptr: i64, v_len: i64) -> i64; // 0 - ok, 1 - binding does not exist
    pub(crate) fn sql_exec(query_ptr: i64, query_len: i64, output_ptr: i64);
    pub(crate) fn sql_batch(query_ptr: i64, query_len: i64, output_ptr: i64);
    pub(crate) fn sql_migrate(migrations_ptr: i64, migrations_len: i64, output_ptr: i64);
    pub(crate) fn queue_push(queue_ptr: i64, queue_len: i64, argument_ptr: i64, argument_len: i64);
    pub(crate) fn log(ptr: i64, len: i64);
    pub(crate) fn fetch(req_ptr: i64, req_len: i64) -> i64;
    pub(crate) fn sleep(millis: i64) -> i64;
    pub(crate) fn random(len: i64, output_ptr: i64);
    pub(crate) fn time() -> i64;
    pub(crate) fn future_poll(index: i64, output_ptr: i64) -> i64; // 0 - pending, 1 - ready
    pub(crate) fn stream_export(output_ptr: i64);
    pub(crate) fn stream_poll_next(index: i64, output_ptr: i64) -> i64; // 0 pending, 1 - ready (some), 2 - ready (none)
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
