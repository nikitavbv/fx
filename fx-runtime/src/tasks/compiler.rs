use {
    tokio::sync::oneshot,
    crate::function::FunctionId,
};

pub(crate) struct CompilerMessage {
    function_id: FunctionId,
    code: Vec<u8>,
    response: oneshot::Sender<wasmtime::Module>,
}
