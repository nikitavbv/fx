use {
    wasmer::FunctionEnvMut,
    tracing::error,
    fx_common::FxFutureError,
    futures::FutureExt,
    crate::{
        runtime::{ExecutionEnv, FunctionId, read_memory_owned},
    },
};

pub fn handle_rpc(
    ctx: FunctionEnvMut<ExecutionEnv>,
    service_name_ptr: i64,
    service_name_len: i64,
    function_name_ptr: i64,
    function_name_len: i64,
    arg_ptr: i64,
    arg_len: i64,
) -> i64 {
    let service_id = FunctionId::new(String::from_utf8(read_memory_owned(&ctx, service_name_ptr, service_name_len)).unwrap());
    let function_name = String::from_utf8(read_memory_owned(&ctx, function_name_ptr, function_name_len)).unwrap();
    let argument = read_memory_owned(&ctx, arg_ptr, arg_len);

    let engine = ctx.data().engine.clone();
    let response_future = match engine.clone().invoke_service_raw(engine.clone(), service_id, function_name, argument) {
        Ok(response_future) => response_future.map(|v| v.map_err(|err| FxFutureError::RpcError {
            reason: err.to_string(),
        })).boxed(),
        Err(err) => std::future::ready(Err(FxFutureError::RpcError { reason: err.to_string() })).boxed(),
    };
    let response_future = match ctx.data().engine.futures_pool.push(response_future.boxed()) {
        Ok(v) => v,
        Err(err) => {
            error!("failed to push future to futures arena: {err:?}");
            // todo: write error object
            return -1;
        }
    };

    response_future.0 as i64
}
