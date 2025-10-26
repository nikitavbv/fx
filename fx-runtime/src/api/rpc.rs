use {
    wasmer::FunctionEnvMut,
    tracing::error,
    fx_common::FxFutureError,
    futures::FutureExt,
    crate::{
        runtime::{ExecutionEnv, FunctionId, read_memory_owned},
    },
};

#[derive(Clone, Copy)]
pub enum ResultCode {
    BindingNotFound = -1,
    RuntimeError = -2,
}

impl ResultCode {
    fn as_i64(&self) -> i64 {
        *self as i64
    }
}

pub fn handle_rpc(
    ctx: FunctionEnvMut<ExecutionEnv>,
    function_id_ptr: i64,
    function_id_len: i64,
    method_name_ptr: i64,
    method_name_len: i64,
    arg_ptr: i64,
    arg_len: i64,
) -> i64 {
    let function_id = FunctionId::new(String::from_utf8(read_memory_owned(&ctx, function_id_ptr, function_id_len)).unwrap());

    let binding = ctx.data().rpc.get(&function_id.as_string());
    if binding.is_none() {
        return ResultCode::BindingNotFound.as_i64();
    }

    let method_name = String::from_utf8(read_memory_owned(&ctx, method_name_ptr, method_name_len)).unwrap();
    let argument = read_memory_owned(&ctx, arg_ptr, arg_len);

    let engine = ctx.data().engine.clone();
    let response_future = match engine.clone().invoke_service_raw(engine.clone(), function_id.clone(), method_name, argument) {
        Ok(response_future) => response_future.map(|v| v
            .map(|v| v.0)
            .map_err(|err| FxFutureError::RpcError {
                reason: err.to_string(),
            })
        ).boxed(),
        Err(err) => std::future::ready(Err(FxFutureError::RpcError { reason: err.to_string() })).boxed(),
    };
    let response_future = match ctx.data().engine.futures_pool.push(response_future.boxed()) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to push future to futures arena: {err:?}");
            return ResultCode::RuntimeError.as_i64();
        }
    };

    engine.metrics.function_fx_api_calls.with_label_values(&[ctx.data().function_id.as_string().as_str(), "rpc"]).inc();

    response_future.0 as i64
}

pub fn handle_send_error(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    ctx.data_mut().execution_error = Some(read_memory_owned(&ctx, addr, len));
}

pub fn handle_send_rpc_response(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    ctx.data_mut().rpc_response = Some(read_memory_owned(&ctx, addr, len));
}
