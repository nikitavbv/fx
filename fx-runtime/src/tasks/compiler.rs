use {
    tokio::sync::oneshot,
    thiserror::Error,
    crate::function::FunctionId,
};

pub(crate) struct CompilerMessage {
    pub(crate) function_id: FunctionId,
    pub(crate) code: Vec<u8>,
    pub(crate) response: oneshot::Sender<Result<wasmtime::Module, CompilerError>>,
}

#[derive(Debug, Error)]
pub(crate) enum CompilerError {
    #[error("failed to compile wasm module")]
    FailedToCompile,
}
