use {
    std::{sync::{Arc, Mutex}, collections::HashMap},
    futures::stream::BoxStream,
    crate::cloud::ExecutionContext,
};

#[derive(Clone)]
pub struct StreamsPool {
    inner: Arc<Mutex<StreamsPoolInner>>,
}

#[derive(Debug)]
pub struct HostPoolIndex(pub u64);

pub enum FxStream {
    HostStream(BoxStream<'static, Vec<u8>>),
    FunctionStream(Arc<ExecutionContext>),
}

impl StreamsPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StreamsPoolInner::new())),
        }
    }

    // push stream owned by host
    pub fn push(&self, stream: BoxStream<'static, Vec<u8>>) -> HostPoolIndex {
        self.inner.lock().unwrap().push(FxStream::HostStream(stream))
    }

    // push stream owned by function
    pub fn push_function_stream(&self, execution_context: Arc<ExecutionContext>) -> HostPoolIndex {
        self.inner.lock().unwrap().push(FxStream::FunctionStream(execution_context))
    }

    // TODO: poll
}

pub struct StreamsPoolInner {
    pool: HashMap<u64, FxStream>,
    counter: u64,
}

impl StreamsPoolInner {
    pub fn new() -> Self {
        Self {
            pool: HashMap::new(),
            counter: 0,
        }
    }

    pub fn push(&mut self, stream: FxStream) -> HostPoolIndex {
        let counter = self.counter;
        self.counter += 1;
        self.pool.insert(counter, stream);
        HostPoolIndex(counter)
    }

    // TODO: poll
}
