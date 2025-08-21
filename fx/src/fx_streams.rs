use {
    std::{sync::{Arc, Mutex}, task::{Context, Poll, Waker}, collections::HashMap},
    futures::{stream::BoxStream, StreamExt, Stream},
    lazy_static::lazy_static,
    fx_common::FxStreamError,
    crate::sys::{self, stream_poll_next, PtrWithLen},
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
        let inner = inner.boxed();
        let output_ptr = sys::PtrWithLen::new();
        unsafe { sys::stream_export(output_ptr.ptr_to_self()) };
        let index: Result<i64, fx_common::FxStreamError> = output_ptr.read_decode();
        let index = index?;
        STREAM_POOL.push(index, inner);
        Ok(Self { index })
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
        let response_ptr = PtrWithLen::new();
        match unsafe { stream_poll_next(self.index, response_ptr.ptr_to_self()) } {
            0 => Poll::Pending,
            1 => Poll::Ready(Some(response_ptr.read_decode())),
            2 => Poll::Ready(None),
            other => panic!("unexpected value: {other:?}"),
        }
    }
}
