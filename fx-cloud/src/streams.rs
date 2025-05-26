use {
    std::{sync::{Arc, Mutex}, collections::HashMap, pin::Pin, task::{self, Poll, Context}, ops::DerefMut},
    futures::{stream::BoxStream, StreamExt},
    crate::{
        cloud::{FxCloud, ServiceId, Engine},
        error::FxCloudError,
    },
};

#[derive(Clone)]
pub struct StreamsPool {
    inner: Arc<Mutex<StreamsPoolInner>>,
}

#[derive(Debug)]
pub struct HostPoolIndex(pub u64);

pub enum FxStream {
    HostStream(BoxStream<'static, Vec<u8>>),
    FunctionStream(ServiceId),
}

impl StreamsPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StreamsPoolInner::new())),
        }
    }

    // push stream owned by host
    pub fn push(&self, stream: BoxStream<'static, Vec<u8>>) -> HostPoolIndex {
        self.inner.lock().unwrap().push(FxStream::HostStream(stream))
    }

    // push stream owned by function
    pub fn push_function_stream(&self, function_id: ServiceId) -> HostPoolIndex {
        self.inner.lock().unwrap().push(FxStream::FunctionStream(function_id))
    }

    pub fn poll_next(&self, engine: Arc<Engine>, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        let mut pool = self.inner.lock().unwrap();
        match pool.poll_next(engine, index, context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(v)) => Poll::Ready(Some(v)),
            Poll::Ready(None) => {
                let _ = pool.remove(index);
                Poll::Ready(None)
            }
        }
    }

    pub fn remove(&self, index: &HostPoolIndex) -> FxStream {
        self.inner.lock().unwrap().remove(index)
    }

    pub fn read(&self, engine: Arc<Engine>, stream: &fx_core::FxStream) -> FxReadableStream {
        FxReadableStream {
            engine,
            stream: self.remove(&HostPoolIndex(stream.index as u64)),
            index: stream.index,
        }
    }

    pub fn len(&self) -> u64 {
        self.inner.lock().unwrap().len()
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

    pub fn poll_next(&mut self, engine: Arc<Engine>, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        poll_next(
            engine,
            index.0 as i64,
            self.pool.get_mut(&index.0).unwrap(),
            context
        ).map(|v| v.map(|v| v.unwrap()))
    }

    pub fn remove(&mut self, index: &HostPoolIndex) -> FxStream {
        self.pool.remove(&index.0).unwrap()
    }

    pub fn len(&self) -> u64 {
        self.pool.len() as u64
    }
}

impl FxCloud {
    pub fn read_stream(&self, stream: &fx_core::FxStream) -> FxReadableStream {
        self.engine.streams_pool.read(self.engine.clone(), stream)
    }
}

pub struct FxReadableStream {
    engine: Arc<Engine>,
    index: i64,
    stream: FxStream,
}

impl futures::Stream for FxReadableStream {
    type Item = Result<Vec<u8>, FxCloudError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        let index = self.index;
        poll_next(self.engine.clone(), index, &mut self.stream, cx)
    }
}

fn poll_next(engine: Arc<Engine>, index: i64, stream: &mut FxStream, cx: &mut task::Context<'_>) -> Poll<Option<Result<Vec<u8>, FxCloudError>>> {
    let function_id = match stream {
        FxStream::HostStream(stream) => return stream.poll_next_unpin(cx).map(|v| v.map(|v| Ok(v))),
        FxStream::FunctionStream(function_id) => function_id,
    };
    engine.stream_poll_next(function_id, index)
}
