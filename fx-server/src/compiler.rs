use {
    std::{sync::{Arc, Mutex, mpsc}, thread, collections::HashSet},
    wasmer::{
        Module,
        Store,
        sys::{CompilerConfig, EngineBuilder},
        wasmparser::Operator,
    },
    wasmer_middlewares::Metering,
    tokio::sync::{mpsc as tokio_mpsc, Mutex as TokioMutex},
    fx_runtime::{
        compiler::{Compiler, CompilerError, BoxedCompiler, MemoizedCompiler, CompilerMetadata},
        runtime::FunctionId,
    },
};

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

    fn compile(&self, _function_id: &FunctionId, bytes: Vec<u8>) -> Result<(Store, Module, CompilerMetadata), CompilerError> {
        let store = self.create_store();
        Module::new(&store, &bytes)
            .map_err(CompilerError::from)
            .map(|module| (store, module, CompilerMetadata {
                backend: "llvm".to_owned(),
            }))
    }
}

pub struct SinglepassCompiler {}

impl SinglepassCompiler {
    pub fn new() -> Self {
        Self {}
    }
}

impl Compiler for SinglepassCompiler {
    fn create_store(&self) -> Store {
        let mut compiler_config = wasmer_compiler_singlepass::Singlepass::default();
        compiler_config.push_middleware(Arc::new(Metering::new(u64::MAX, ops_cost_function)));
        Store::new(EngineBuilder::new(compiler_config))
    }

    fn compile(&self, _function_id: &FunctionId, module_code: Vec<u8>) -> Result<(Store, Module, CompilerMetadata), CompilerError> {
        let store = self.create_store();
        Module::new(&store, &module_code)
            .map_err(CompilerError::from)
            .map(|module| (store, module, CompilerMetadata {
                backend: "singlepass".to_owned(),
            }))
    }
}

fn ops_cost_function(_: &Operator) -> u64 { 1 }

#[derive(Clone)]
pub struct TieredCompiler {
    fast: Arc<BoxedCompiler>,
    optimizing: Arc<MemoizedCompiler>,
    optimizing_tx: Arc<mpsc::Sender<(FunctionId, Vec<u8>)>>,
    pending: Arc<Mutex<HashSet<Vec<u8>>>>,
    optimized_rx: Arc<TokioMutex<tokio_mpsc::Receiver<FunctionId>>>,
}

impl TieredCompiler {
    pub fn new(fast: BoxedCompiler, optimizing: MemoizedCompiler) -> Self {
        let fast = Arc::new(fast);
        let optimizing = Arc::new(optimizing);
        let pending = Arc::new(Mutex::new(HashSet::new()));

        let (optimizing_tx, rx) = mpsc::channel::<(FunctionId, Vec<u8>)>();
        let (optimized_tx, optimized_rx) = tokio_mpsc::channel(100);
        {
            let optimizing = optimizing.clone();
            let pending = pending.clone();

            thread::spawn(move || {
                while let Ok((function_id, module_code)) = rx.recv() {
                    let _ = optimizing.compile(&function_id, module_code.clone());

                    if let Ok(mut pending) = pending.lock() {
                        pending.remove(&module_code);
                    }
                    optimized_tx.blocking_send(function_id).unwrap();
                }
            });
        }

        Self {
            fast,
            optimizing,
            optimizing_tx: Arc::new(optimizing_tx),
            pending,
            optimized_rx: Arc::new(TokioMutex::new(optimized_rx)),
        }
    }

    pub fn consume_optimizations(&self) -> Arc<TokioMutex<tokio_mpsc::Receiver<FunctionId>>> {
        self.optimized_rx.clone()
    }
}

impl Compiler for TieredCompiler {
    fn create_store(&self) -> Store {
        // the reason why create_store cannot be called directly is because we cannot know which
        // compiler will be used ahead of time.
        panic!("create_store should not be called on TieredCompiler, use compile instead")
    }

    fn compile(&self, function_id: &FunctionId, module_code: Vec<u8>) -> Result<(Store, Module, CompilerMetadata), CompilerError> {
        if let Some((store, module, metadata)) = self.optimizing.load_from_storage_if_available(&module_code)? {
            return Ok((store, module, CompilerMetadata {
                backend: format!("tiered[optimized] | {}", metadata.backend),
            }));
        }

        if let Ok(mut pending) = self.pending.lock() {
            if !pending.contains(&module_code) {
                pending.insert(module_code.clone());
                self.optimizing_tx.send((function_id.clone(), module_code.clone())).unwrap();
            }
        }

        self.fast.compile(function_id, module_code)
            .map(|(store, module, metadata)| (store, module, CompilerMetadata {
                backend: format!("tiered[fast] | {}", metadata.backend),
            }))
    }
}
