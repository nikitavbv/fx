use {
    std::{sync::{Arc, Mutex}, collections::HashMap},
    futures::future::BoxFuture,
};

#[derive(Clone)]
pub struct FuturesPool {
    inner: Arc<Mutex<FuturesPoolInner>>,
}

#[derive(Debug)]
pub struct HostPoolIndex(u64);

impl FuturesPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FuturesPoolInner::new())),
        }
    }

    pub fn push(&self, future: BoxFuture<'static, Vec<u8>>) -> HostPoolIndex {
        self.inner.lock().unwrap().push(future)
    }
}

struct FuturesPoolInner {
    pool: HashMap<u64, BoxFuture<'static, Vec<u8>>>,
    counter: u64,
}

impl FuturesPoolInner {
    pub fn new() -> Self {
        Self {
            pool: HashMap::new(),
            counter: 0,
        }
    }

    pub fn push(&mut self, future: BoxFuture<'static, Vec<u8>>) -> HostPoolIndex {
        let counter = self.counter;
        self.counter += 1;
        self.pool.insert(counter, future);
        HostPoolIndex(counter)
    }
}
