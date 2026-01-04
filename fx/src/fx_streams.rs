use {
    std::{sync::{Arc, Mutex}, task::{Context, Poll, Waker}, collections::HashMap},
    tracing::info,
    futures::{stream::BoxStream, StreamExt, Stream},
    lazy_static::lazy_static,
    fx_common::FxStreamError,
    fx_api::{fx_capnp, capnp},
    crate::{
        sys::{self, PtrWithLen},
        invoke_fx_api,
    },
};

pub use fx_common::FxStream;

lazy_static! {
    pub(crate) static ref STREAM_POOL: StreamPool = StreamPool::new();
}

pub(crate) struct StreamPool {
    pool: Arc<Mutex<PoolInner>>,
}

struct PoolInner {
    streams: HashMap<i64, BoxStream<'static, Vec<u8>>>,
}

impl StreamPool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(Mutex::new(PoolInner::new())),
        }
    }

    pub fn push(&self, index: i64, stream: BoxStream<'static, Vec<u8>>) {
        self.pool.lock().unwrap().push(index, stream);
    }

    pub fn next(&self, index: i64) -> Poll<Option<Vec<u8>>> {
        let mut context = Context::from_waker(Waker::noop());
        let mut pool = self.pool.lock().unwrap();
        match pool.next(index, &mut context) {
            Poll::Ready(None) => {
                pool.remove(index);
                Poll::Ready(None)
            },
            Poll::Ready(Some(v)) => Poll::Ready(Some(v)),
            Poll::Pending => Poll::Pending,
        }
    }

    pub fn remove(&self, index: u64) {
        self.pool.lock().unwrap().remove(index as i64);
    }
}

impl PoolInner {
    pub fn new() -> Self {
        Self {
            streams: HashMap::new(),
        }
    }

    pub fn push(&mut self, index: i64, stream: BoxStream<'static, Vec<u8>>) {
        self.streams.insert(index, stream);
    }

    pub fn next(&mut self, index: i64, context: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        self.streams.get_mut(&index).as_mut().unwrap().poll_next_unpin(context)
    }

    pub fn remove(&mut self, index: i64) {
        let _ = self.streams.remove(&index);
    }
}

pub trait FxStreamExport {
    fn wrap(inner: impl Stream<Item = Vec<u8>> + Send + 'static) -> Result<Self, fx_common::FxStreamError> where Self: Sized;
}

impl FxStreamExport for FxStream {
    fn wrap(inner: impl Stream<Item = Vec<u8>> + Send + 'static) -> Result<Self, fx_common::FxStreamError> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let _stream_export_request = op.init_stream_export();

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::StreamExport(v) => {
                let stream_export_response = v.unwrap();
                match stream_export_response.get_response().which().unwrap() {
                    fx_capnp::stream_export_response::response::Which::StreamId(index) => {
                        let index = index as i64;
                        STREAM_POOL.push(index, inner.boxed());
                        Ok(Self { index })
                    },
                    fx_capnp::stream_export_response::response::Which::Error(err) => Err(FxStreamError::PushFailed {
                        reason: err.unwrap().to_string().unwrap(),
                    }),
                }
            },
            _other => panic!("unexpected response from stream export api"),
        }
    }
}

pub trait FxStreamImport {
    fn import(self) -> FxImportedStream;
}

impl FxStreamImport for FxStream {
    fn import(self) -> FxImportedStream {
        FxImportedStream { index: self.index }
    }
}

pub struct FxImportedStream {
    index: i64,
}

impl Stream for FxImportedStream {
    type Item = Result<Vec<u8>, FxStreamError>;
    fn poll_next(self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut stream_poll_request = op.init_stream_poll_next();
        stream_poll_request.set_stream_id(self.index as u64);

        let response = invoke_fx_api(message);
        let response = response.get_root::<fx_capnp::fx_api_call_result::Reader>().unwrap();

        match response.get_op().which().unwrap() {
            fx_capnp::fx_api_call_result::op::Which::StreamPollNext(v) => {
                let stream_poll_response = v.unwrap();
                match stream_poll_response.get_response().which().unwrap() {
                    fx_capnp::stream_poll_next_response::response::Which::Pending(_) => Poll::Pending,
                    fx_capnp::stream_poll_next_response::response::Which::Ready(v) => Poll::Ready(Some({
                        let v = v.unwrap();
                        match v.get_item().which().unwrap() {
                            fx_capnp::stream_poll_next_response::stream_item::item::Which::Result(v) => Ok(v.unwrap().to_vec()),
                            fx_capnp::stream_poll_next_response::stream_item::item::Which::Error(err) => Err(FxStreamError::PollFailed {
                                reason: err.unwrap().to_string().unwrap(),
                            }),
                        }
                    })),
                    fx_capnp::stream_poll_next_response::response::Which::Finished(_) => Poll::Ready(None),
                }
            },
            _other => panic!("unexpected response from stream poll api"),
        }
    }
}
