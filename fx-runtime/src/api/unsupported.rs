use {
    wasmer::{FunctionEnvMut, Value, RuntimeError},
    tracing::error,
    crate::runtime::ExecutionEnv,
};

pub fn handle_unsupported(ctx: FunctionEnvMut<ExecutionEnv>, _vals: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    error!(function=ctx.data().function_id.as_string(), "function called unsupported api");
    Err(RuntimeError::new("this api is not supported by fx"))
}
