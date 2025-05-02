use {
    std::{sync::{Arc, Mutex}, task::{Poll, Context}, collections::HashMap},
    futures::{future::BoxFuture, FutureExt},
};

#[derive(Clone)]
pub struct FuturesPool {
    inner: Arc<Mutex<FuturesPoolInner>>,
}

#[derive(Debug)]
pub struct HostPoolIndex(pub u64);

impl FuturesPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FuturesPoolInner::new())),
        }
    }

    pub fn push(&self, future: BoxFuture<'static, Vec<u8>>) -> HostPoolIndex {
        self.inner.lock().unwrap().push(future)
    }

    pub fn poll(&self, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Vec<u8>> {
        let mut pool = self.inner.lock().unwrap();
        match pool.poll(index, context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                pool.remove(index);
                Poll::Ready(result)
            }
        }
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

    pub fn poll(&mut self, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Vec<u8>> {
        self.pool.get_mut(&index.0).unwrap().poll_unpin(context)
    }

    pub fn remove(&mut self, index: &HostPoolIndex) {
        let _ = self.pool.remove(&index.0).unwrap();
    }
}
