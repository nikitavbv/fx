use {
    std::{sync::{Arc, Mutex}, task::{Context, Poll, Waker}, collections::HashMap},
    futures::{stream::BoxStream, StreamExt, Stream},
    lazy_static::lazy_static,
    serde::{Serialize, Deserialize},
};

lazy_static! {
    pub(crate) static ref STREAM_POOL: StreamPool = StreamPool::new();
}

pub(crate) struct StreamPool {
    pool: Arc<Mutex<PoolInner>>,
}

struct PoolInner {
    streams: HashMap<u64, BoxStream<'static, Vec<u8>>>,
    index: u64,
}

#[derive(Serialize)]
pub(crate) struct StreamPoolIndex(pub(crate) u64);

impl StreamPool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(Mutex::new(PoolInner::new())),
        }
    }

    pub fn push(&self, stream: BoxStream<'static, Vec<u8>>) -> StreamPoolIndex {
        self.pool.lock().unwrap().push(stream)
    }

    pub fn next(&self, index: StreamPoolIndex) -> Poll<Option<Vec<u8>>> {
        let mut context = Context::from_waker(Waker::noop());
        let mut pool = self.pool.lock().unwrap();
        match pool.next(&index, &mut context) {
            Poll::Ready(None) => {
                pool.remove(&index);
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
            index: 0,
        }
    }

    pub fn push(&mut self, stream: BoxStream<'static, Vec<u8>>) -> StreamPoolIndex {
        let index = self.index;
        self.index += 1;
        self.streams.insert(index, stream);
        StreamPoolIndex(index)
    }

    pub fn next(&mut self, index: &StreamPoolIndex, context: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        self.streams.get_mut(&index.0).as_mut().unwrap().poll_next_unpin(context)
    }

    pub fn remove(&mut self, index: &StreamPoolIndex) {
        let _ = self.streams.remove(&index.0).unwrap();
    }
}

#[derive(Serialize, Deserialize)]
pub struct FxStream {
    // TODO: FxStream should create stream on host side. On host side, each entry in StreamPool should contain reference to ExecutionContext or wrapped stream
    // created on host side (i.e., body of `fetch`). when stream is polled, host should run poll_next function in that ExecutionContext or poll_next a local Stream (depending on where stream lives).
    // One more positive aspect of this implementation is that it allows passthrough of streams (for example, from fetch to HttpResponse) - poll_next on host will poll Stream on host side without
    // invoking function and copying data there.
    // index: StreamPoolIndex,
}

impl FxStream {
    pub fn wrap(inner: impl Stream<Item = Vec<u8>> + Send + 'static) -> Self {
        let inner = inner.boxed();
        let index = STREAM_POOL.push(inner);
        unimplemented!()
        // Self { index }
    }
}

// TODO: host stream
