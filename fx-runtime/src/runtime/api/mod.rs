use {
    std::{
        time::{SystemTime, UNIX_EPOCH},
        task::{self, Poll},
    },
    tracing::error,
    thiserror::Error,
    futures::FutureExt,
    wasmtime::{AsContext, AsContextMut},
    rand::TryRngCore,
    fx_types::{capnp, abi_capnp},
    fx_common::FxFutureError,
    crate::runtime::{
        runtime::{ExecutionEnv, write_memory_obj, PtrWithLen, FunctionId, FunctionExecutionError},
        kv::KVStorage,
        error::FunctionInvokeError,
        logs::{self, Logger},
    },
};

// TODO:
// - rate limiting - use governor crate and have a set of rate limits defined in FunctionDefinition
// - permissions - based on capabilities
// - metrics - counters per syscall type
// - no "fx_cloud" namespace, should be just one more binding

/// Error returned by an async api that is then wrapped by HostFuture
#[derive(Error, Debug)]
pub enum HostFutureAsyncApiError {
    /// Error caused by rpc api that is being wrapped
    #[error("rpc api error: {0:?}")]
    Rpc(#[from] RpcApiAsyncError),
}

pub fn fx_api_handler(mut caller: wasmtime::Caller<'_, crate::runtime::runtime::ExecutionEnv>, req_addr: i64, req_len: i64, output_ptr: i64) {
    let memory = caller.get_export("memory").map(|v| v.into_memory().unwrap()).unwrap();

    let context = caller.as_context();
    let view = memory.data(&context);
    let req_addr = req_addr as usize;
    let req_len = req_len as usize;

    let mut message_bytes = &view[req_addr..req_addr+req_len];
    let message_reader = fx_types::capnp::serialize::read_message_from_flat_slice(&mut message_bytes, fx_types::capnp::message::ReaderOptions::default()).unwrap();
    let request = message_reader.get_root::<fx_types::abi_capnp::fx_api_call::Reader>().unwrap();
    let op = request.get_op();

    let mut response_message = capnp::message::Builder::new_default();
    let response = response_message.init_root::<abi_capnp::fx_api_call_result::Builder>();
    let mut response_op = response.init_op();

    use fx_types::abi_capnp::fx_api_call::op::{Which as Operation};
    match op.which().unwrap() {
        Operation::MetricsCounterIncrement(v) => {
            handle_metrics_counter_increment(caller.data(), v.unwrap());
            response_op.set_metrics_counter_increment(());
        },
        Operation::Deprecated1(_) => panic!("call to deprecated api"),
        Operation::KvGet(v) => {
            handle_kv_get(caller.data(), v.unwrap(), response_op.init_kv_get());
        },
        Operation::KvSet(v) => {
            unimplemented!("deprecated")
        },
        Operation::SqlExec(v) => {
            unimplemented!("deprecated");
        },
        Operation::SqlBatch(v) => {
            unimplemented!("deprecated");
        },
        Operation::SqlMigrate(v) => {
            unimplemented!("deprecated");
        },
        Operation::Log(v) => {
            unimplemented!("deprecated");
        },
        Operation::Fetch(v) => {
            unimplemented!("deprecated");
        },
        Operation::Sleep(v) => {
            unimplemented!("deprecated");
        },
        Operation::Random(v) => {
            unimplemented!("deprecated");
        },
        Operation::Time(_) => {
            unimplemented!("deprecated");
        },
        Operation::FuturePoll(v) => {
            unimplemented!("deprecated")
        },
        Operation::FutureDrop(v) => {
            unimplemented!("deprecated")
        },
        Operation::StreamExport(v) => {
            unimplemented!("deprecated")
        },
        Operation::StreamPollNext(v) => {
            unimplemented!("deprecated")
        }
    };

    let response_size = capnp::serialize::compute_serialized_size_in_words(&response_message) * 8;

    let fx_malloc = caller.data().instance.as_ref().unwrap().clone();

    let ptr = fx_malloc.borrow().get_typed_func::<i64, i64>(caller.as_context_mut(), "_fx_malloc").unwrap()
        .call(caller.as_context_mut(), response_size as i64)
        .unwrap() as usize;

    let view = memory.data_mut(caller.as_context_mut());

    unsafe {
        capnp::serialize::write_message(&mut view[ptr..ptr+response_size], &response_message).unwrap();
    }

    write_memory_obj(view, output_ptr, PtrWithLen { ptr: ptr as i64, len: response_size as i64 });
}

fn handle_metrics_counter_increment(data: &ExecutionEnv, counter_increment_request: abi_capnp::metrics_counter_increment_request::Reader) {
    let result = data.engine.metrics.function_metrics.counter_increment(
        &data.function_id,
        counter_increment_request.get_counter_name().unwrap().to_str().unwrap(),
        counter_increment_request.get_tags().unwrap()
            .into_iter()
            .map(|v| (v.get_name().unwrap().to_string().unwrap(), v.get_value().unwrap().to_string().unwrap()))
            .collect(),
        counter_increment_request.get_delta()
    );

    if let Err(err) = result {
        error!("failed to increment counter: {err:?}");
    }
}

#[derive(Error, Debug)]
enum RpcApiAsyncError {
    #[error("failed to execute function: {0:?}")]
    FunctionInvocation(#[from] FunctionExecutionError),
}

fn handle_kv_get(data: &ExecutionEnv, kv_get_request: abi_capnp::kv_get_request::Reader, kv_get_response: abi_capnp::kv_get_response::Builder) {
    let mut kv_get_response = kv_get_response.init_response();

    let binding = kv_get_request.get_binding_id().unwrap().to_str().unwrap();
    let storage = match data.storage.get(binding) {
        Some(v) => v,
        None => {
            kv_get_response.set_binding_not_found(());
            return;
        }
    };

    let key = kv_get_request.get_key().unwrap();
    let value = storage.get(key).unwrap();
    let value = match value {
        Some(v) => v,
        None => {
            kv_get_response.set_key_not_found(());
            return;
        }
    };

    kv_get_response.set_value(&value);

    data.engine.metrics.function_fx_api_calls.with_label_values(&[data.function_id.as_string().as_str(), "kv::get"]).inc();
}
