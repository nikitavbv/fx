use {
    std::sync::Arc,
    wasmer::{
        Module,
        Store,
        sys::{Cranelift, CompilerConfig, EngineBuilder},
        wasmparser::Operator,
    },
    wasmer_middlewares::Metering,
    sha2::{Sha256, Digest},
    thiserror::Error,
    crate::{
        kv::{KVStorage, BoxedStorage},
        runtime::FunctionId,
    },
};

pub trait Compiler {
    fn create_store(&self) -> Store;
    fn compile(&self, function_id: &FunctionId, bytes: Vec<u8>) -> Result<(Store, Module, CompilerMetadata), CompilerError>;
}

#[derive(Clone)]
pub struct CompilerMetadata {
    pub backend: String,
}

#[derive(Error, Debug)]
pub enum CompilerError {
    #[error("failed to compile: {reason}")]
    FailedToCompile { reason: String },

    #[error("failed to deserialize: {reason}")]
    FailedToDeserialize { reason: String },
}

#[derive(Clone)]
pub struct BoxedCompiler {
    inner: Arc<Box<dyn Compiler + Send + Sync>>,
}

impl BoxedCompiler {
    pub fn new<T: Compiler + Send + Sync + 'static>(inner: T) -> Self {
        Self {
            inner: Arc::new(Box::new(inner)),
        }
    }
}

impl Compiler for BoxedCompiler {
    fn create_store(&self) -> Store {
        self.inner.create_store()
    }

    fn compile(&self, function_id: &FunctionId, bytes: Vec<u8>) -> Result<(Store, Module, CompilerMetadata), CompilerError> {
        self.inner.compile(function_id, bytes)
    }
}

pub struct CraneliftCompiler {}

impl CraneliftCompiler {
    pub fn new() -> Self {
        Self {}
    }
}

impl Compiler for CraneliftCompiler {
    fn create_store(&self) -> Store {
        let mut compiler_config = Cranelift::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, ops_cost_function)));
        Store::new(EngineBuilder::new(compiler_config))
    }

    fn compile(&self, _function_id: &FunctionId, bytes: Vec<u8>) -> Result<(Store, Module, CompilerMetadata), CompilerError> {
        let store = self.create_store();
        Module::new(&store, &bytes)
            .map_err(|err| CompilerError::FailedToCompile { reason: err.to_string() })
            .map(|module| (store, module, CompilerMetadata {
                backend: "cranelift".to_owned(),
            }))
    }
}

fn ops_cost_function(_: &Operator) -> u64 { 1 }

pub struct MemoizedCompiler {
    storage: BoxedStorage,
    compiler: BoxedCompiler,
}

impl MemoizedCompiler {
    #[allow(dead_code)]
    pub fn new(storage: BoxedStorage, compiler: BoxedCompiler) -> Self {
        Self {
            storage,
            compiler,
        }
    }

    fn key(&self, module_code: &[u8]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(module_code);
        hasher.finalize().to_vec()
    }

    pub fn load_from_storage_if_available(&self, module_code: &[u8]) -> Result<Option<(Store, Module, CompilerMetadata)>, CompilerError> {
        let key = self.key(&module_code);
        self.storage.get(&key)
            .unwrap()
            .map(|compiled_module| {
                let store = self.create_store();
                let module = unsafe { Module::deserialize(&store, compiled_module) };
                module.map_err(|err| CompilerError::FailedToDeserialize { reason: err.to_string() })
                    .map(|module| (store, module, CompilerMetadata {
                        backend: "unknown".to_owned(),
                    }))
            })
            .transpose()
    }
}

// TODO: Module supports .clone()
impl Compiler for MemoizedCompiler {
    fn create_store(&self) -> Store {
        self.compiler.create_store()
    }

    fn compile(&self, function_id: &FunctionId, module_code: Vec<u8>) -> Result<(Store, Module, CompilerMetadata), CompilerError> {
        if let Some((store, module, compiler_metadata)) = self.load_from_storage_if_available(&module_code)? {
            return Ok((store, module, CompilerMetadata {
                backend: format!("MemoizedCompiler[hit] | {}", compiler_metadata.backend),
            }));
        };

        let key = self.key(&module_code);
        let (store, module, compiler_metadata) = self.compiler.compile(function_id, module_code)?;
        let serialized = module.serialize().unwrap();
        self.storage.set(&key, &serialized.to_vec()).unwrap();
        Ok((store, module, CompilerMetadata {
            backend: format!("MemoizedCompiler[miss] | {}", compiler_metadata.backend),
        }))
    }
}
