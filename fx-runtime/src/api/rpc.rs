use {
    wasmer::FunctionEnvMut,
    crate::{
        runtime::{ExecutionEnv, read_memory_owned},
    },
};

pub fn handle_send_error(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    ctx.data_mut().execution_error = Some(read_memory_owned(&ctx, addr, len));
}

pub fn handle_send_rpc_response(mut ctx: FunctionEnvMut<ExecutionEnv>, addr: i64, len: i64) {
    ctx.data_mut().rpc_response = Some(read_memory_owned(&ctx, addr, len));
}
