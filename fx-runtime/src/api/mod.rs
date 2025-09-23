// TODO: see rpc and refactor all other api calls similarly
pub(crate) mod kv;
pub(crate) mod rpc;

// TODO:
// - rate limiting - use governor crate and have a set of rate limits defined in FunctionDefinition
// - permissions - based on capabilities
// - metrics - counters per syscall type
// - no "fx_cloud" namespace, should be just one more binding
