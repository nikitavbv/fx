use {
    std::{sync::{Arc, RwLock, Mutex}, task::{Poll, Context}, collections::{HashMap, VecDeque}},
    futures::{future::BoxFuture, FutureExt},
    thiserror::Error,
    fx_common::FxFutureError,
    crate::api::HostFutureAsyncApiError,
};

#[derive(Clone)]
pub struct FuturesPool {
    pool: Arc<RwLock<HashMap<u64, Arc<Mutex<BoxFuture<'static, Result<Vec<u8>, HostFutureError>>>>>>>,
    futures_to_drop: Arc<Mutex<VecDeque<u64>>>,
    counter: Arc<Mutex<u64>>,
}

#[derive(Debug)]
pub struct HostPoolIndex(pub u64);

/// Error returned by host future when you poll it
#[derive(Error, Debug)]
pub enum HostFutureError {
    /// Error returned by an async api that this future wraps.
    #[error("async api error: {0:?}")]
    AsyncApiError(#[from] HostFutureAsyncApiError),
}

/// Error returned by futures runtime when you try to poll host future.
/// Polling can fail because of error in surrounding infrastructure or if the future itself returned an error.
#[derive(Error, Debug)]
pub enum HostFuturePollError {
    /// Error caused by runtime implementation of the futures.
    /// Should never happen. If you see this error it means there is a bug somewhere.
    #[error("error in runtime implementation of the futures: {0:?}")]
    Runtime(#[from] HostFuturePollRuntimeError),

    /// Future with this ID is not present in the arena.
    #[error("future not found")]
    NotFound,

    /// Future itself returned an error
    #[error("future error: {0:?}")]
    FutureError(#[from] HostFutureError),
}

/// Error in the runtime implementation of the futures.
/// Should never happen. If you see this error it means there is a bug somewhere.
#[derive(Error, Debug)]
pub enum HostFuturePollRuntimeError {
    #[error("failed to acquire lock for futures arena: {reason:?}")]
    ArenaFailedToLock {
        reason: String,
    },

    #[error("failed to acquire lock for specific future in the arena: {reason:?}")]
    FutureFailedToLock {
        reason: String,
    },
}

/// legacy error
#[derive(Error, Debug)]
pub enum FuturesError {
    #[error("failed to acquire arena lock: {reason:?}")]
    LockingError {
        reason: String,
    }
}

impl FuturesPool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(Mutex::new(0)),
            futures_to_drop: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn push(&self, future: BoxFuture<'static, Result<Vec<u8>, HostFutureError>>) -> Result<HostPoolIndex, FxFutureError> {
        let counter = {
            let mut counter = self.counter.try_lock()
                .map_err(|err| FxFutureError::FxRuntimeError {
                    reason: format!("failed to acquire lock for futures arena counter: {err:?}"),
                })?;
            *counter += 1;
            *counter
        };
        self.pool.try_write()
            .map_err(|err| FxFutureError::FxRuntimeError {
                reason: format!("failed to acquire lock for futures arena: {err:?}"),
            })?
            .insert(counter, Arc::new(Mutex::new(future)));
        Ok(HostPoolIndex(counter))
    }

    pub fn poll(&self, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Result<Vec<u8>, HostFuturePollError>> {
        let future = self.pool.try_read()
            .map_err(|err| HostFuturePollRuntimeError::ArenaFailedToLock {
                reason: err.to_string(),
            })?
            .get(&index.0)
            .ok_or(HostFuturePollError::NotFound)?
            .clone();
        let mut future = match future.try_lock() {
            Ok(v) => v,
            Err(err) => {
                return Poll::Ready(Err(HostFuturePollError::from(HostFuturePollRuntimeError::FutureFailedToLock {
                    reason: err.to_string(),
                })))
            },
        };

        let result = match future.poll_unpin(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                drop(future);
                match self.pool.try_write() {
                    Ok(mut v) => {
                        v.remove(&index.0);

                        // cleanup dropped futures
                        if let Ok(mut futures_to_drop) = self.futures_to_drop.try_lock() {
                            while let Some(future_to_drop) = futures_to_drop.pop_front() {
                                v.remove(&future_to_drop);
                            }
                        }
                    },
                    Err(err) => {
                        return Poll::Ready(Err(HostFuturePollError::from(HostFuturePollRuntimeError::ArenaFailedToLock {
                            reason: err.to_string(),
                        })));
                    }
                };
                Poll::Ready(result)
            }
        };

        result.map_err(|err| HostFuturePollError::from(err))
    }

    pub fn remove(&self, index: &HostPoolIndex) {
        self.futures_to_drop.lock().unwrap().push_back(index.0);
    }

    pub fn len(&self) -> Result<u64, FuturesError> {
        self.pool.try_read()
            .map(|v| v.len() as u64)
            .map_err(|err| FuturesError::LockingError {
                reason: format!("failed to acquire arena lock: {err:?}"),
            })
    }
}
