use {
    std::sync::Arc,
    wasmer::{Module, Store},
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
