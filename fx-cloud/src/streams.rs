use {
    std::{sync::{Arc, Mutex}, collections::HashMap},
    futures::stream::BoxStream,
};

#[derive(Clone)]
pub struct StreamsPool {
    inner: Arc<Mutex<StreamsPoolInner>>,
}

#[derive(Debug)]
pub struct HostPoolIndex(pub u64);

impl StreamsPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StreamsPoolInner::new())),
        }
    }

    pub fn push(&self, stream: BoxStream<'static, Vec<u8>>) -> HostPoolIndex {
        self.inner.lock().unwrap().push(stream)
    }

    // TODO: poll
}

pub struct StreamsPoolInner {
    pool: HashMap<u64, BoxStream<'static, Vec<u8>>>,
    counter: u64,
}

impl StreamsPoolInner {
    pub fn new() -> Self {
        Self {
            pool: HashMap::new(),
            counter: 0,
        }
    }

    pub fn push(&mut self, stream: BoxStream<'static, Vec<u8>>) -> HostPoolIndex {
        let counter = self.counter;
        self.counter += 1;
        self.pool.insert(counter, stream);
        HostPoolIndex(counter)
    }

    // TODO: poll
}
