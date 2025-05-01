use {
    std::sync::{Mutex, Arc},
    std::task::Context,
    std::collections::HashMap,
    futures::{FutureExt, future::BoxFuture},
    lazy_static::lazy_static,
};

lazy_static! {
    static ref FUTURE_POOL: FuturePool = FuturePool::new();
}

struct FuturePool {
    pool: Arc<Mutex<PoolInner>>,
}

struct PoolInner {
    futures: HashMap<u64, BoxFuture<'static, Vec<u8>>>,
    index: u64,
}

struct PoolIndex(u64);

impl FuturePool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(Mutex::new(PoolInner::new())),
        }
    }

    pub fn push(&self, future: BoxFuture<'static, Vec<u8>>) -> PoolIndex {
        self.pool.lock().unwrap().push(future)
    }
}

impl PoolInner {
    pub fn new() -> Self {
        Self {
            futures: HashMap::new(),
            index: 0,
        }
    }

    pub fn push(&mut self, future: BoxFuture<'static, Vec<u8>>) -> PoolIndex {
        let index = self.index;
        self.index += 1;
        self.futures.insert(index, future);
        PoolIndex(index)
    }

    pub fn poll(&mut self, index: &PoolIndex, context: &mut Context<'_>) -> std::task::Poll<Vec<u8>> {
        self.futures.get_mut(&index.0).as_mut().unwrap().poll_unpin(context)
    }
}

pub struct FxFuture {
}

impl FxFuture {
    pub fn wrap<T: serde::ser::Serialize>(inner: impl Future<Output = T> + Send + 'static) -> Self {
        let inner = inner.map(|v| rmp_serde::to_vec(&v).unwrap()).boxed();
        FUTURE_POOL.push(inner);
        Self {
        }
    }
}

pub struct FxHostFuture {
    index: PoolIndex,
}
