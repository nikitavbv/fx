use {
    std::{sync::{Arc, Mutex}, collections::HashMap, pin::Pin, task::{self, Poll, Context}},
    futures::{stream::BoxStream, StreamExt},
    crate::{
        runtime::{FxRuntime, FunctionId, Engine},
        error::FxRuntimeError,
    },
};

// streams can leak memory. Each stream must be closed by host or functions, because tracking ownership accross host/functions is very tricky.
// automatic cleanup (i.e., implementing Drop trait) is up to high-level wrappers.

#[derive(Clone)]
pub struct StreamsPool {
    inner: Arc<Mutex<StreamsPoolInner>>,
}

#[derive(Debug, Clone)]
pub struct HostPoolIndex(pub u64);

pub enum FxStream {
    HostStream(BoxStream<'static, Vec<u8>>),
    FunctionStream(FunctionId),
}

impl StreamsPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StreamsPoolInner::new())),
        }
    }

    // push stream owned by host
    pub fn push(&self, stream: BoxStream<'static, Vec<u8>>) -> Result<HostPoolIndex, FxRuntimeError> {
        Ok(self.inner.lock()
            .map_err(|err| FxRuntimeError::StreamingError { reason: format!("lock is poisoned: {err:?}") })?
            .push(FxStream::HostStream(stream)))
    }

    // push stream owned by function
    pub fn push_function_stream(&self, function_id: FunctionId) -> Result<HostPoolIndex, FxRuntimeError> {
        Ok(self.inner.lock()
            .map_err(|err| FxRuntimeError::StreamingError { reason: format!("lock is poisoned: {err:?}") })?
            .push(FxStream::FunctionStream(function_id)))
    }

    pub fn poll_next(&self, engine: Arc<Engine>, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Option<Result<Vec<u8>, FxRuntimeError>>> {
        let mut pool = match self.inner.lock() {
            Ok(v) => v,
            Err(err) => return Poll::Ready(Some(Err(FxRuntimeError::StreamingError { reason: format!("lock is poisoned: {err:?}") }))),
        };

        match pool.poll_next(engine, index, context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(v)) => Poll::Ready(Some(v)),
            Poll::Ready(None) => {
                let _ = pool.remove(index);
                Poll::Ready(None)
            }
        }
    }

    pub fn remove(&self, index: &HostPoolIndex) -> Result<Option<FxStream>, FxRuntimeError> {
        self.inner
            .lock()
            .map_err(|err| FxRuntimeError::StreamingError { reason: format!("lock is poisoned: {err:?}") })
            .map(|mut v| v.remove(index))
    }

    pub fn read(&self, engine: Arc<Engine>, stream: &fx_common::FxStream) -> Result<Option<FxReadableStream>, FxRuntimeError> {
        Ok(Some(FxReadableStream {
            engine,
            stream: match self.remove(&HostPoolIndex(stream.index as u64))? {
                Some(v) => v,
                None => return Ok(None),
            },
            index: stream.index,
        }))
    }

    pub fn len(&self) -> Result<u64, FxRuntimeError> {
        self.inner.lock()
            .map_err(|err| FxRuntimeError::StreamingError { reason: format!("lock is poisoned: {err:?}") })
            .map(|v| v.len())
    }
}

pub struct StreamsPoolInner {
    pool: HashMap<u64, FxStream>,
    counter: u64,
}

impl StreamsPoolInner {
    pub fn new() -> Self {
        Self {
            pool: HashMap::new(),
            counter: 0,
        }
    }

    pub fn push(&mut self, stream: FxStream) -> HostPoolIndex {
        let counter = self.counter;
        self.counter += 1;
        self.pool.insert(counter, stream);
        HostPoolIndex(counter)
    }

    pub fn poll_next(&mut self, engine: Arc<Engine>, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Option<Result<Vec<u8>, FxRuntimeError>>> {
        let stream = match self.pool.get_mut(&index.0) {
            Some(v) => v,
            None => return Poll::Ready(Some(Err(FxRuntimeError::StreamNotFound))),
        };

        poll_next(
            engine,
            index.0 as i64,
            stream,
            context
        )
    }

    pub fn remove(&mut self, index: &HostPoolIndex) -> Option<FxStream> {
        self.pool.remove(&index.0)
    }

    pub fn len(&self) -> u64 {
        self.pool.len() as u64
    }
}

impl FxRuntime {
    #[allow(dead_code)]
    pub fn read_stream(&self, stream: &fx_common::FxStream) -> Result<Option<FxReadableStream>, FxRuntimeError> {
        self.engine.streams_pool.read(self.engine.clone(), stream)
    }
}

pub struct FxReadableStream {
    engine: Arc<Engine>,
    index: i64,
    stream: FxStream,
}

impl futures::Stream for FxReadableStream {
    type Item = Result<Vec<u8>, FxRuntimeError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        let index = self.index;
        poll_next(self.engine.clone(), index, &mut self.stream, cx)
    }
}

impl Drop for FxReadableStream {
    fn drop(&mut self) {
        match &self.stream {
            FxStream::HostStream(_) => {
                // nothing to do, memory on host will be free-d automatically
            },
            FxStream::FunctionStream(function_id) => {
                self.engine.stream_drop(function_id, self.index);
            }
        }
    }
}

fn poll_next(engine: Arc<Engine>, index: i64, stream: &mut FxStream, cx: &mut task::Context<'_>) -> Poll<Option<Result<Vec<u8>, FxRuntimeError>>> {
    let function_id = match stream {
        FxStream::HostStream(stream) => return stream.poll_next_unpin(cx).map(|v| v.map(|v| Ok(v))),
        FxStream::FunctionStream(function_id) => function_id,
    };
    engine.stream_poll_next(function_id, index)
}
