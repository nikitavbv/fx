use {
    std::{sync::{Arc, RwLock, Mutex}, task::{Poll, Context}, collections::HashMap},
    futures::{future::BoxFuture, FutureExt},
    fx_core::FxFutureError,
};

#[derive(Clone)]
pub struct FuturesPool {
    pool: Arc<RwLock<HashMap<u64, Arc<Mutex<BoxFuture<'static, Result<Vec<u8>, FxFutureError>>>>>>>,
    counter: Arc<Mutex<u64>>,
}

#[derive(Debug)]
pub struct HostPoolIndex(pub u64);

impl FuturesPool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(Mutex::new(0)),
        }
    }

    pub fn push(&self, future: BoxFuture<'static, Result<Vec<u8>, FxFutureError>>) -> HostPoolIndex {
        let counter = {
            let mut counter = self.counter.try_lock().unwrap();
            *counter += 1;
            *counter
        };
        self.pool.try_write().unwrap().insert(counter, Arc::new(Mutex::new(future)));
        HostPoolIndex(counter)
    }

    pub fn poll(&self, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Result<Vec<u8>, FxFutureError>> {
        let future = self.pool.try_read().unwrap().get(&index.0).unwrap().clone();
        let mut future = future.try_lock().unwrap();

        match future.poll_unpin(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                drop(future);
                self.pool.try_write().unwrap().remove(&index.0);
                Poll::Ready(result)
            }
        }
    }

    pub fn len(&self) -> u64 {
        self.pool.try_read().unwrap().len() as u64
    }
}
