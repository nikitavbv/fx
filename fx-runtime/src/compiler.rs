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
    crate::kv::{KVStorage, BoxedStorage},
};

pub trait Compiler {
    fn create_store(&self) -> Store;
    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Result<Module, CompilerError>;
}

#[derive(Error, Debug)]
pub enum CompilerError {
    #[error("failed to compile: {reason}")]
    FailedToCompile { reason: String },

    #[error("failed to deserialize: {reason}")]
    FailedToDeserialize { reason: String },
}

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

    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Result<Module, CompilerError> {
        self.inner.compile(store, bytes)
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

    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Result<Module, CompilerError> {
        Module::new(&store, &bytes).map_err(|err| CompilerError::FailedToCompile { reason: err.to_string() })
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
}

// TODO: Module supports .clone()
impl Compiler for MemoizedCompiler {
    fn create_store(&self) -> Store {
        self.compiler.create_store()
    }

    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Result<Module, CompilerError> {
        let key = self.key(&bytes);
        match self.storage.get(&key).unwrap() {
            Some(v) => {
                let module = unsafe { Module::deserialize(store, v) };
                module.map_err(|err| CompilerError::FailedToDeserialize { reason: err.to_string() })
            },
            None => {
                let module = self.compiler.compile(store, bytes)?;
                let serialized = module.serialize().unwrap();
                self.storage.set(&key, &serialized.to_vec()).unwrap();
                Ok(module)
            }
        }
    }
}
