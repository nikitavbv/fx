use {
    std::sync::Arc,
    wasmer::{
        Module,
        Store,
        sys::{Cranelift, CompilerConfig, EngineBuilder},
        wasmparser::Operator,
    },
    wasmer_middlewares::Metering,
    fx_runtime::compiler::{Compiler, CompilerError},
};

pub struct CraneliftCompiler {}

impl CraneliftCompiler {
    pub fn new() -> Self {
        Self {}
    }
}

impl Compiler for CraneliftCompiler {
    fn create_wasmer_engine_builder(&self) -> EngineBuilder {
        let mut compiler_config = Cranelift::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, ops_cost_function)));
        EngineBuilder::new(compiler_config)
    }

    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Result<Module, CompilerError> {
        Module::new(&store, &bytes).map_err(|err| CompilerError::FailedToCompile { reason: err.to_string() })
    }
}

pub struct LLVMCompiler {}

impl LLVMCompiler {
    #[cfg(not(target_arch = "aarch64"))]
    pub fn new() -> Option<Self> {
        Some(Self {})
    }

    #[cfg(target_arch = "aarch64")]
    pub fn new() -> Option<Self> {
        None
    }
}

impl Compiler for LLVMCompiler {
    #[cfg(not(target_arch = "aarch64"))]
    fn create_wasmer_engine_builder(&self) -> wasmer::sys::EngineBuilder {
        let mut compiler_config = wasmer_compiler_llvm::LLVM::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, ops_cost_function)));
        EngineBuilder::new(compiler_config)
    }

    #[cfg(target_arch = "aarch64")]
    fn create_wasmer_engine_builder(&self) -> wasmer::sys::EngineBuilder {
        unreachable!("should not be reachable on aarch64")
    }

    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Result<Module, CompilerError> {
        Module::new(&store, &bytes).map_err(|err| CompilerError::FailedToCompile { reason: err.to_string() })
    }
}

fn ops_cost_function(_: &Operator) -> u64 { 1 }
