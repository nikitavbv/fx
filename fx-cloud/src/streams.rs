use {
    std::{sync::{Arc, Mutex}, collections::HashMap, pin::Pin, task::{self, Poll, Context}, ops::DerefMut},
    futures::{stream::BoxStream, StreamExt},
    crate::{
        cloud::{ExecutionContext, FxCloud},
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
    FunctionStream(Arc<ExecutionContext>),
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
    pub fn push_function_stream(&self, execution_context: Arc<ExecutionContext>) -> HostPoolIndex {
        self.inner.lock().unwrap().push(FxStream::FunctionStream(execution_context))
    }

    pub fn poll_next(&self, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        let mut pool = self.inner.lock().unwrap();
        match pool.poll_next(index, context) {
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

    pub fn poll_next(&mut self, index: &HostPoolIndex, context: &mut Context<'_>) -> Poll<Option<Vec<u8>>> {
        poll_next(
            index.0 as i64,
            self.pool.get_mut(&index.0).unwrap(),
            context
        ).map(|v| v.map(|v| v.unwrap()))
    }

    pub fn remove(&mut self, index: &HostPoolIndex) -> FxStream {
        self.pool.remove(&index.0).unwrap()
    }
}

impl FxCloud {
    pub fn read_stream(&self, stream: &fx_core::FxStream) -> FxReadableStream {
        FxReadableStream {
            index: stream.index,
            stream: self.engine.streams_pool.remove(&HostPoolIndex(stream.index as u64)),
        }
    }
}

pub struct FxReadableStream {
    index: i64,
    stream: FxStream,
}

impl futures::Stream for FxReadableStream {
    type Item = Result<Vec<u8>, FxCloudError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        let index = self.index;
        poll_next(index, &mut self.stream, cx)
    }
}

fn poll_next(index: i64, stream: &mut FxStream, cx: &mut task::Context<'_>) -> Poll<Option<Result<Vec<u8>, FxCloudError>>> {
    let ctx = match stream {
        FxStream::HostStream(stream) => return stream.poll_next_unpin(cx).map(|v| v.map(|v| Ok(v))),
        FxStream::FunctionStream(execution_context) => execution_context,
    };
    let mut store_lock = ctx.store.lock().unwrap();
    let store = store_lock.deref_mut();

    let function_stream_next = ctx.instance.exports.get_function("_fx_stream_next").unwrap();
    // TODO: measure points
    let poll_next = function_stream_next.call(store, &[wasmer::Value::I64(index)]).unwrap()[0].unwrap_i64();
    match poll_next {
        0 => Poll::Pending,
        1 => {
            let response = ctx.function_env.as_ref(store).rpc_response.as_ref().unwrap().clone();
            Poll::Ready(Some(Ok(response)))
        },
        2 => Poll::Ready(None),
        other => Poll::Ready(Some(Err(FxCloudError::StreamingError {
            reason: format!("unexpected repsonse code from _fx_stream_next: {other:?}"),
        }))),
    }
}
