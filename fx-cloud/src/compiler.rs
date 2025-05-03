use {
    std::sync::Arc,
    wasmer::{Module, Store},
    sha2::{Sha256, Digest},
    crate::storage::{KVStorage, BoxedStorage},
};

pub trait Compiler {
    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Module;
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
    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Module {
        self.inner.compile(store, bytes)
    }
}

pub struct SimpleCompiler;

impl SimpleCompiler {
    pub fn new() -> Self {
        Self
    }
}

impl Compiler for SimpleCompiler {
    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Module {
        Module::new(&store, &bytes).unwrap()
    }
}

pub struct MemoizedCompiler {
    storage: BoxedStorage,
    compiler: BoxedCompiler,
}

impl MemoizedCompiler {
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
    fn compile(&self, store: &Store, bytes: Vec<u8>) -> Module {
        let key = self.key(&bytes);
        match self.storage.get(&key).unwrap() {
            Some(v) => {
                let module = unsafe { Module::deserialize(store, v) };
                module.unwrap()
            },
            None => {
                let module = self.compiler.compile(store, bytes);
                let serialized = module.serialize().unwrap();
                self.storage.set(&key, &serialized.to_vec()).unwrap();
                module
            }
        }
    }
}
