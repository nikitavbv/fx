use {
    std::{sync::{Arc, Mutex}, task::{Context, Poll, Waker}, collections::HashMap},
    futures::{stream::BoxStream, StreamExt, Stream},
    lazy_static::lazy_static,
    crate::sys::{self, stream_poll_next, PtrWithLen},
};

pub use fx_core::FxStream;

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
        let _ = self.streams.remove(&index).unwrap();
    }
}

pub trait FxStreamExport {
    fn wrap(inner: impl Stream<Item = Vec<u8>> + Send + 'static) -> Self;
}

impl FxStreamExport for FxStream {
    fn wrap(inner: impl Stream<Item = Vec<u8>> + Send + 'static) -> Self {
        let inner = inner.boxed();
        let index = unsafe { sys::stream_export() };
        STREAM_POOL.push(index, inner);
        Self { index }
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
    type Item = Vec<u8>;
    fn poll_next(self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let response_ptr = PtrWithLen::new();
        match unsafe { stream_poll_next(self.index, response_ptr.ptr_to_self()) } {
            0 => Poll::Pending,
            1 => Poll::Ready(Some(response_ptr.read_owned())),
            2 => Poll::Ready(None),
            other => panic!("unexpected value: {other:?}"),
        }
    }
}
