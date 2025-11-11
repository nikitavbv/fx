use {
    wasmer::FunctionEnvMut,
    tracing::info,
    crate::runtime::ExecutionEnv,
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

pub fn fx_api_handler(ctx: FunctionEnvMut<ExecutionEnv>, req_addr: i64, req_len: i64) {
    let function_id = ctx.data().function_id.clone();

    let memory = ctx.data().memory.as_ref().unwrap();
    let view = memory.view(&ctx);
    let req_addr = req_addr as usize;
    let req_len = req_len as usize;
    let mut data = unsafe { &view.data_unchecked_mut()[req_addr..req_addr+req_len] };

    let message_reader = fx_api::capnp::serialize::read_message_from_flat_slice(&mut data, fx_api::capnp::message::ReaderOptions::default()).unwrap();
    let request = message_reader.get_root::<fx_api::fx_capnp::fx_api_call::Reader>().unwrap();
    let op = request.get_op();

    use fx_api::fx_capnp::fx_api_call::op::{Which as Operation};
    match op.which().unwrap() {
        Operation::MetricsCounterIncrement(v) => {
            let counter_increment_request = v.unwrap();
            ctx.data().engine.metrics.function_metrics.counter_increment(&function_id, counter_increment_request.get_counter_name().unwrap().to_str().unwrap(), counter_increment_request.get_delta());
        },
        Operation::Rpc(v) => {
            unimplemented!()
        }
    };
}
