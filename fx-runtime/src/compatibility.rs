use {
    wasmer::{FunctionEnvMut, Value, RuntimeError},
    crate::runtime::ExecutionEnv,
};

pub fn api_unsupported(_ctx: FunctionEnvMut<ExecutionEnv>, _vals: &[Value]) -> Result<Vec<Value>, RuntimeError> {
    unimplemented!("this compatibility is not implemented yet")
}
