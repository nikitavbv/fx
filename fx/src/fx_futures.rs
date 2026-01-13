use {
    std::{sync::{Mutex, Arc}, task::{Context, Poll, Waker}, collections::HashMap},
    tracing::{info, error},
    thiserror::Error,
    futures::{FutureExt, future::BoxFuture},
    lazy_static::lazy_static,
    serde::Serialize,
    fx_api::{fx_capnp, capnp},
    crate::{PtrWithLen, error::FxError, invoke_fx_api},
};

/// Errors that may be returned by a function future when you poll it
#[derive(Error, Debug)]
pub enum FunctionFutureError {
    /// error produced by user's implementation of the handler
    #[error("user application error: {description}")]
    UserApplicationError {
        description: String,
    }
}

/// Errors that may be returned by a future imported from the host when you poll it
#[derive(Error, Debug)]
pub enum HostFutureError {
    /// Error caused by runtime implementation of futures.
    /// Should never happen. If you see this error it means there is a bug somewhere.
    #[error("error in runtime implementation of the futures: {0:?}")]
    RuntimeError(#[from] HostFuturePollRuntimeError),

    /// Error returned by an async api that host future wraps.
    #[error("async api error: {0:?}")]
    AsyncApiError(#[from] HostFutureAsyncApiError),
}

/// Error in the runtime implementation of the futures.
/// Should never happen. If you see this error it means there is a bug somewhere.
#[derive(Error, Debug)]
pub enum HostFuturePollRuntimeError {
    /// Unknown runtime error on the host side.
    #[error("runtime error on the host side")]
    HostRuntimeError,
}

/// Error returned by an async api that is then wrapped by HostFuture
#[derive(Error, Debug)]
pub enum HostFutureAsyncApiError {
    /// Error in "fetch" async api
    #[error("fetch api error: {0:?}")]
    Fetch(FetchApiAsyncError),

    /// Error in "rcp" async api
    #[error("rpc api error: {0:?}")]
    Rpc(RpcApiAsyncError),
}

#[derive(Error, Debug)]
pub enum FetchApiAsyncError {
    #[error("network error")]
    NetworkError,
}

#[derive(Error, Debug)]
pub enum RpcApiAsyncError {
    #[error("failed to execute target function")]
    TargetFunctionExecutionError,
}

lazy_static! {
    pub(crate) static ref FUTURE_POOL: FuturePool = FuturePool::new();
}

pub(crate) struct FuturePool {
    pool: Arc<Mutex<PoolInner>>,
}

struct PoolInner {
    futures: HashMap<u64, BoxFuture<'static, Result<Vec<u8>, FunctionFutureError>>>,
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

    pub fn push(&self, future: BoxFuture<'static, Result<Vec<u8>, FunctionFutureError>>) -> PoolIndex {
        self.pool.lock().unwrap().push(future)
    }

    pub fn poll(&self, index: PoolIndex) -> Poll<Result<Vec<u8>, FunctionFutureError>> {
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

    pub fn push(&mut self, future: BoxFuture<'static, Result<Vec<u8>, FunctionFutureError>>) -> PoolIndex {
        let index = self.index;
        self.index += 1;
        self.futures.insert(index, future);
        PoolIndex(index)
    }

    pub fn poll(&mut self, index: &PoolIndex, context: &mut Context<'_>) -> Poll<Result<Vec<u8>, FunctionFutureError>> {
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
    pub fn wrap(inner: impl Future<Output = Result<Vec<u8>, FunctionFutureError>> + Send + 'static) -> Self {
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
    type Output = Result<Vec<u8>, HostFutureError>;
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
                    fx_capnp::future_poll_response::response::Which::Error(error) => Poll::Ready(Err({
                        let error = error.unwrap();
                        match error.get_error().which().unwrap() {
                            fx_capnp::future_poll_response::future_poll_error::error::Which::RuntimeError(_) => HostFutureError::RuntimeError(
                                HostFuturePollRuntimeError::HostRuntimeError
                            ),
                            fx_capnp::future_poll_response::future_poll_error::error::Which::AsyncApiError(async_api_error) => {
                                HostFutureError::AsyncApiError(match async_api_error.unwrap().get_op().which().unwrap() {
                                    fx_capnp::future_poll_response::future_poll_error::async_api_error::op::Which::Fetch(_) => {
                                        HostFutureAsyncApiError::Fetch(FetchApiAsyncError::NetworkError)
                                    },
                                    fx_capnp::future_poll_response::future_poll_error::async_api_error::op::Which::Rpc(err) => {
                                        HostFutureAsyncApiError::Rpc(match err.unwrap().get_error().which().unwrap() {
                                            // TODO: add all variants
                                        })
                                    },
                                })
                            },
                            fx_capnp::future_poll_response::future_poll_error::error::Which::NotFound(_) => {},
                        }
                    })),
                }
            },
            _other => panic!("unexpected response from future_poll api"),
        }
    }
}

impl Drop for FxHostFuture {
    fn drop(&mut self) {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut future_drop_request = op.init_future_drop();
        future_drop_request.set_future_id(self.index.0);

        let _response = invoke_fx_api(message);
    }
}
