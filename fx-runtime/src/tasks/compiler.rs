use {
    tokio::sync::oneshot,
    crate::function::FunctionId,
};

pub(crate) struct CompilerMessage {
    pub(crate) function_id: FunctionId,
    pub(crate) code: Vec<u8>,
    pub(crate) response: oneshot::Sender<wasmtime::Module>,
}
