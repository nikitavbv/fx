use {
    std::{sync::{Mutex, Arc}, task::{Context, Poll, Waker}, collections::HashMap},
    tracing::{info, error},
    futures::{FutureExt, future::BoxFuture},
    lazy_static::lazy_static,
    serde::Serialize,
    fx_common::FxFutureError,
    fx_api::{fx_capnp, capnp},
    crate::{sys::future_drop, PtrWithLen, error::FxError, invoke_fx_api},
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
        let mut pool = self.pool.lock()
            .expect("failed to lock future arena when polling future");
        match pool.poll(&index, &mut context) {
            Poll::Ready(v) => {
                if let Err(err) = pool.remove(&index) {
                    error!("failed to remove future from arena: {err:?}");
                };
                Poll::Ready(v)
            },
            Poll::Pending => Poll::Pending
        }
    }

    pub fn remove(&self, index: PoolIndex) -> Result<(), FxError> {
        info!(index=index.0, "removing future");

        self.pool
            .try_lock()
            .map_err(|err| FxError::FutureError {
                reason: format!("failed to lock future pool: {err:?}"),
            })?
            .remove(&index)
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
        self.futures.get_mut(&index.0).as_mut()
            .expect("failed to lock futures arena in PoolInner")
            .poll_unpin(context)
    }

    pub fn remove(&mut self, index: &PoolIndex) -> Result<(), FxError> {
        match self.futures.remove(&index.0) {
            Some(v) => {
                drop(v);
                Ok(())
            },
            None => Err(FxError::FutureError { reason: format!("future with this index does not exist in the pool") })
        }
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
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut future_poll_request = op.init_future_poll();
        future_poll_request.set_future_id(self.index.0);

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::FuturePoll(v) => {
                let future_poll_response = v.unwrap();
                match future_poll_response.get_response().which().unwrap() {
                    fx_capnp::future_poll_response::response::Which::Pending(_) => Poll::Pending,
                    fx_capnp::future_poll_response::response::Which::Result(v) => Poll::Ready(Ok(v.unwrap().to_vec())),
                    fx_capnp::future_poll_response::response::Which::Error(err) => Poll::Ready(Err(FxFutureError::FxRuntimeError { reason: err.unwrap().to_string().unwrap() })),
                }
            },
            _other => panic!("unexpected response from future_poll api"),
        }
    }
}

impl Drop for FxHostFuture {
    fn drop(&mut self) {
        unsafe {
            future_drop(self.index.0 as i64)
        }
    }
}
