use {
    wasmer::FunctionEnvMut,
    crate::runtime::{ExecutionEnv, read_memory_owned},
};

pub fn handle_metrics_counter_increment(
    ctx: FunctionEnvMut<ExecutionEnv>,
    counter_name_addr: i64,
    counter_name_len: i64,
    delta: i64
) {
    let function_id = ctx.data().function_id.clone();
    let counter_name = String::from_utf8(read_memory_owned(&ctx, counter_name_addr, counter_name_len)).unwrap();
    ctx.data().engine.metrics.function_metrics.counter_increment(&function_id, &counter_name, delta as u64);
}
