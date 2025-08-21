use {
    std::{sync::{Mutex, Arc}, task::{Context, Poll, Waker}, collections::HashMap},
    futures::{FutureExt, future::BoxFuture},
    lazy_static::lazy_static,
    serde::Serialize,
    fx_common::FxFutureError,
    crate::{sys::future_poll, PtrWithLen},
};

lazy_static! {
    pub(crate) static ref FUTURE_POOL: FuturePool = FuturePool::new();
}

pub(crate) struct FuturePool {
    pool: Arc<Mutex<PoolInner>>,
}

struct PoolInner {
    futures: HashMap<u64, BoxFuture<'static, Vec<u8>>>,
    index: u64,
}

#[derive(Serialize)]
pub(crate) struct PoolIndex(pub(crate) u64);

impl FuturePool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(Mutex::new(PoolInner::new())),
        }
    }

    pub fn push(&self, future: BoxFuture<'static, Vec<u8>>) -> PoolIndex {
        self.pool.lock().unwrap().push(future)
    }

    pub fn poll(&self, index: PoolIndex) -> Poll<Vec<u8>> {
        let mut context = Context::from_waker(Waker::noop());
        let mut pool = self.pool.lock().unwrap();
        match pool.poll(&index, &mut context) {
            Poll::Ready(v) => {
                pool.remove(&index);
                Poll::Ready(v)
            },
            Poll::Pending => Poll::Pending
        }
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

    pub fn poll(&mut self, index: &PoolIndex, context: &mut Context<'_>) -> Poll<Vec<u8>> {
        self.futures.get_mut(&index.0).as_mut().unwrap().poll_unpin(context)
    }

    pub fn remove(&mut self, index: &PoolIndex) {
        drop(self.futures.remove(&index.0).unwrap());
    }
}

#[derive(Serialize)]
pub struct FxFuture {
    index: PoolIndex,
}

impl FxFuture {
    pub fn wrap(inner: impl Future<Output = Vec<u8>> + Send + 'static) -> Self {
        let inner = inner.boxed();
        let index = FUTURE_POOL.push(inner);
        Self {
            index,
        }
    }

    pub fn future_index(&self) -> u64 {
        self.index.0
    }
}

pub struct FxHostFuture {
    index: PoolIndex,
    response_ptr: PtrWithLen,
}

impl FxHostFuture {
    pub(crate) fn new(index: PoolIndex) -> Self {
        Self {
            index,
            response_ptr: PtrWithLen::new(),
        }
    }
}

impl Future for FxHostFuture {
    type Output = Result<Vec<u8>, FxFutureError>;
    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if unsafe { future_poll(self.index.0 as i64, self.response_ptr.ptr_to_self()) } == 0 {
            Poll::Pending
        } else {
            Poll::Ready(rmp_serde::from_slice(&self.response_ptr.read_owned()).unwrap())
        }
    }
}
