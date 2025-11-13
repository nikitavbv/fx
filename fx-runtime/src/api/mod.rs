use {
    wasmer::FunctionEnvMut,
    tracing::error,
    futures::FutureExt,
    fx_api::{capnp, fx_capnp},
    fx_common::FxFutureError,
    crate::runtime::{ExecutionEnv, write_memory_obj, PtrWithLen, FunctionId},
};

// TODO: see rpc and refactor all other api calls similarly
pub(crate) mod kv;
pub(crate) mod rpc;
pub(crate) mod sql;
pub(crate) mod unsupported;

// TODO:
// - rate limiting - use governor crate and have a set of rate limits defined in FunctionDefinition
// - permissions - based on capabilities
// - metrics - counters per syscall type
// - no "fx_cloud" namespace, should be just one more binding

pub fn fx_api_handler(mut ctx: FunctionEnvMut<ExecutionEnv>, req_addr: i64, req_len: i64, output_ptr: i64) {
    let function_id = ctx.data().function_id.clone();

    let (data, mut store) = ctx.data_and_store_mut();

    let memory = data.memory.as_ref().unwrap();
    let view = memory.view(&store);
    let req_addr = req_addr as usize;
    let req_len = req_len as usize;

    let mut message_bytes = unsafe { &view.data_unchecked_mut()[req_addr..req_addr+req_len] };
    let message_reader = fx_api::capnp::serialize::read_message_from_flat_slice(&mut message_bytes, fx_api::capnp::message::ReaderOptions::default()).unwrap();
    let request = message_reader.get_root::<fx_api::fx_capnp::fx_api_call::Reader>().unwrap();
    let op = request.get_op();

    let mut response_message = capnp::message::Builder::new_default();
    let response = response_message.init_root::<fx_capnp::fx_api_call_result::Builder>();
    let mut response_op = response.init_op();

    use fx_api::fx_capnp::fx_api_call::op::{Which as Operation};
    match op.which().unwrap() {
        Operation::MetricsCounterIncrement(v) => {
            let counter_increment_request = v.unwrap();
            data.engine.metrics.function_metrics.counter_increment(&function_id, counter_increment_request.get_counter_name().unwrap().to_str().unwrap(), counter_increment_request.get_delta());
            response_op.set_metrics_counter_increment(());
        },
        Operation::Rpc(v) => {
            handle_rpc(data, v.unwrap(), response_op.init_rpc());
        }
    };

    let response_size = capnp::serialize::compute_serialized_size_in_words(&response_message) * 8;
    let ptr = data.client_malloc().call(&mut store, &[wasmer::Value::I64(response_size as i64)]).unwrap()[0].i64().unwrap() as usize;

    unsafe {
        capnp::serialize::write_message(&mut memory.view(&store).data_unchecked_mut()[ptr..ptr+response_size], &response_message).unwrap();
    }

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr: ptr as i64, len: response_size as i64 });
}

fn handle_rpc<'s>(data: &ExecutionEnv, rpc_request: fx_capnp::rpc_call_request::Reader, rpc_response: fx_capnp::rpc_call_response::Builder) {
    let mut rpc_response = rpc_response.init_response();

    let function_id = FunctionId::new(rpc_request.get_function_id().unwrap().to_string().unwrap());
    if data.rpc.get(&function_id.as_string()).is_none() {
        rpc_response.set_binding_not_found(());
    };

    let method_name = rpc_request.get_method_name().unwrap().to_str().unwrap();
    let argument = rpc_request.get_argument().unwrap();

    let engine = data.engine.clone();
    let response_future = match engine.clone().invoke_service_raw(engine.clone(), function_id.clone(), method_name.to_owned(), argument.to_vec()) {
        Ok(response_future) => response_future.map(|v| v
            .map(|v| v.0)
            .map_err(|err| FxFutureError::RpcError {
                reason: err.to_string(),
            })
        ).boxed(),
        Err(err) => std::future::ready(Err(FxFutureError::RpcError { reason: err.to_string() })).boxed(),
    };
    let response_future = match data.engine.futures_pool.push(response_future.boxed()) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to push future to futures arena: {err:?}");
            rpc_response.set_runtime_error(());
            return;
        }
    };

    engine.metrics.function_fx_api_calls.with_label_values(&[data.function_id.as_string().as_str(), "rpc"]).inc();

    rpc_response.set_future_id(response_future.0);
}
