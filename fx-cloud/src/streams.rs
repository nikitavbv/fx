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

#[derive(Clone, Debug)]
pub struct HostPoolIndex(pub u64);

pub enum FxStream {
    HostStream {
        stream: BoxStream<'static, Vec<u8>>,
        ownership: StreamOwnership,
    },
    FunctionStream {
        function_id: ServiceId,
        ownership: StreamOwnership,
    }
}

pub enum StreamOwnership {
    Host,
    Function(ServiceId),
}

pub struct HostStreamGuard {
    pool: StreamsPool,
    index: HostPoolIndex,
}

impl StreamsPool {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StreamsPoolInner::new())),
        }
    }

    // push stream owned by host
    pub fn push(&self, stream: BoxStream<'static, Vec<u8>>) -> (HostPoolIndex, HostStreamGuard) {
        let index = self.inner.lock().unwrap().push(FxStream::HostStream {
            stream,
            ownership: StreamOwnership::Host,
        });
        (index.clone(), HostStreamGuard { pool: self.clone(), index })
    }

    // push stream owned by function
    pub fn push_function_stream(&self, function_id: ServiceId) -> HostPoolIndex {
        self.inner.lock().unwrap().push(FxStream::FunctionStream { ownership: StreamOwnership::Function(function_id.clone()), function_id })
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

    pub fn transfer_ownership(&self, index: &HostPoolIndex, function_id: ServiceId) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(stream) = inner.pool.get_mut(&index.0) {
            match stream {
                FxStream::HostStream { stream: _, ownership } => {
                    *ownership = StreamOwnership::Function(function_id);
                },
                FxStream::FunctionStream { function_id: _, ownership: _ } => panic!("this stream is already owned by a different function"),
            }
        }
    }

    pub fn drop_from_host(&self, index: &HostPoolIndex) {
        let mut inner = self.inner.lock().unwrap();
        let is_owned_by_host = if let Some(stream) = inner.pool.get(&index.0) {
            match stream {
                FxStream::HostStream { stream: _, ownership } => {
                    match ownership {
                        StreamOwnership::Function(_) => {
                            // owned by function, do nothing
                            false
                        },
                        StreamOwnership::Host => {
                            // owned by host
                            true
                        }
                    }
                },
                FxStream::FunctionStream { function_id: _, ownership: _ } => {
                    // do nothing from function stream
                    false
                }
            }
        } else {
            false
        };

        if is_owned_by_host {
            inner.pool.remove(&index.0);
        }
    }

    pub fn drop_from_function(&self, index: &HostPoolIndex) {
        let mut inner = self.inner.lock().unwrap();
        let is_owned_by_function = if let Some(stream) = inner.pool.get(&index.0) {
            match stream {
                FxStream::HostStream { stream: _, ownership } => {
                    match ownership {
                        StreamOwnership::Function(_) => true,
                        StreamOwnership::Host => {
                            panic!("cannot drop stream: it is owned by host");
                        }
                    }
                },
                FxStream::FunctionStream { function_id: _, ownership: _ } => {
                    panic!("should not be dropping function stream");
                }
            }
        } else {
            false
        };

        if is_owned_by_function {
            inner.pool.remove(&index.0);
        }
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

impl Drop for HostStreamGuard {
    fn drop(&mut self) {
        self.pool.drop_from_host(&self.index)
    }
}

fn poll_next(engine: Arc<Engine>, index: i64, stream: &mut FxStream, cx: &mut task::Context<'_>) -> Poll<Option<Result<Vec<u8>, FxCloudError>>> {
    let function_id = match stream {
        FxStream::HostStream { stream, ownership: _ } => return stream.poll_next_unpin(cx).map(|v| v.map(|v| Ok(v))),
        FxStream::FunctionStream { function_id, ownership: _ } => function_id,
    };
    engine.stream_poll_next(function_id, index)
}
