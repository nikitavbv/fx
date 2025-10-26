use {
    std::{sync::{Arc, Mutex, mpsc}, thread, collections::HashSet},
    wasmer::{
        Module,
        Store,
        sys::{Cranelift, CompilerConfig, EngineBuilder},
        wasmparser::Operator,
    },
    wasmer_middlewares::Metering,
    fx_runtime::compiler::{Compiler, CompilerError, BoxedCompiler, MemoizedCompiler},
};

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

    fn compile(&self, bytes: Vec<u8>) -> Result<(Store, Module), CompilerError> {
        let store = self.create_store();
        Module::new(&store, &bytes)
            .map_err(|err| CompilerError::FailedToCompile { reason: err.to_string() })
            .map(|module| (store, module))
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
    fn create_store(&self) -> Store {
        let mut compiler_config = wasmer_compiler_llvm::LLVM::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, ops_cost_function)));
        Store::new(EngineBuilder::new(compiler_config))
    }

    #[cfg(target_arch = "aarch64")]
    fn create_store(&self) -> Store {
        unreachable!("should not be reachable on aarch64")
    }

    fn compile(&self, bytes: Vec<u8>) -> Result<(Store, Module), CompilerError> {
        let store = self.create_store();
        Module::new(&store, &bytes)
            .map_err(|err| CompilerError::FailedToCompile { reason: err.to_string() })
            .map(|module| (store, module))
    }
}

fn ops_cost_function(_: &Operator) -> u64 { 1 }

pub struct TieredCompiler {
    fast: BoxedCompiler,
    optimizing: Arc<MemoizedCompiler>,
    optimizing_tx: mpsc::Sender<Vec<u8>>,
    pending: Arc<Mutex<HashSet<Vec<u8>>>>,
}

impl TieredCompiler {
    pub fn new(fast: BoxedCompiler, optimizing: MemoizedCompiler) -> Self {
        let optimizing = Arc::new(optimizing);
        let pending = Arc::new(Mutex::new(HashSet::new()));

        let (optimizing_tx, rx) = mpsc::channel::<Vec<u8>>();
        {
            let optimizing = optimizing.clone();
            let pending = pending.clone();

            thread::spawn(move || {
                while let Ok(module_code) = rx.recv() {
                    let _ = optimizing.compile(module_code.clone());

                    if let Ok(mut pending) = pending.lock() {
                        pending.remove(&module_code);
                    }
                }
            });
        }

        Self {
            fast,
            optimizing,
            optimizing_tx,
            pending,
        }
    }
}

impl Compiler for TieredCompiler {
    fn create_store(&self) -> Store {
        // the reason why create_store cannot be called directly is because we cannot know which
        // compiler will be used ahead of time.
        panic!("create_store should not be called on TieredCompiler, use compile instead")
    }

    fn compile(&self, module_code: Vec<u8>) -> Result<(Store, Module), CompilerError> {
        if let Some(v) = self.optimizing.load_from_storage_if_available(&module_code)? {
            return Ok(v);
        }

        if let Ok(mut pending) = self.pending.lock() {
            if !pending.contains(&module_code) {
                pending.insert(module_code.clone());
                self.optimizing_tx.send(module_code.clone()).unwrap();
            }
        }

        self.fast.compile(module_code)
    }
}
