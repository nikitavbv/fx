use {
    std::task::Poll,
    futures::{stream::BoxStream, FutureExt, StreamExt},
    hyper::body::Bytes,
    crate::{
        function::abi::{capnp, abi_http_capnp},
        resources::{serialize::{SerializeResource, SerializableResource}, ResourceId, resource::OwnedFunctionResourceId},
        triggers::http::{HttpBody, FunctionResourceReader},
    },
};

pub(crate) enum FetchResult {
    Inline(FetchResultInline),
    BodyResource(SerializableResource<FetchResultWithBodyResource>),
}

impl FetchResult {
    pub fn new(parts: http::response::Parts, body: HttpBody) -> Self {
        Self::Inline(FetchResultInline::new(parts, body))
    }
}

pub(crate) struct FetchResultInline {
    parts: http::response::Parts,
    body: HttpBody,
}

impl FetchResultInline {
    pub fn new(parts: http::response::Parts, body: HttpBody) -> Self {
        Self {
            parts,
            body,
        }
    }

    pub fn into_parts(self) -> (http::response::Parts, HttpBody) {
        (self.parts, self.body)
    }
}

pub(crate) struct FetchResultWithBodyResource {
    parts: http::response::Parts,
    body: ResourceId,
}

impl FetchResultWithBodyResource {
    pub fn new(parts: http::response::Parts, body: ResourceId) -> Self {
        Self { parts, body }
    }
}

impl SerializeResource for FetchResultWithBodyResource {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let mut fetch_response = message.init_root::<abi_http_capnp::http_response::Builder>();

        fetch_response.set_status(self.parts.status.as_u16());

        let mut headers = fetch_response.reborrow().init_headers(self.parts.headers.len() as u32);
        for (index, (name, value)) in self.parts.headers.iter().enumerate() {
            let mut header = headers.reborrow().get(index as u32);
            header.set_name(name.as_str());
            header.set_value(value.to_str().unwrap());
        }

        fetch_response.set_body_resource_id(self.body.as_u64());

        capnp::serialize::write_message_to_words(&message)
    }
}

pub(crate) enum HttpStreamFrame {
    Bytes(Vec<u8>),
    Stream(BoxStream<'static, Result<Bytes, ()>>),
}

pub(crate) fn poll_function_resource_reader_frame(mut reader: FunctionResourceReader, cx: &mut std::task::Context<'_>) -> (
    FunctionResourceReader,
    Poll<Option<Result<Bytes, std::io::Error>>>,
) {
    // first of all, handle cases that do not involve reading from function
    reader = match reader {
        FunctionResourceReader::Empty => return (FunctionResourceReader::Empty, Poll::Ready(None)),
        FunctionResourceReader::Stream(mut stream) => {
            let frame = stream.poll_next_unpin(cx);
            return (FunctionResourceReader::Stream(stream), frame.map_err(|_| {
                todo!("unhandled error");
            }));
        },
        other => other,
    };

    // if finished reading, then finish this body
    reader = match reader {
        FunctionResourceReader::Empty => return (reader, Poll::Ready(None)),
        other => other,
    };

    // if there is a HttpBody resource we can start reading, let's request next frame
    reader = match reader {
        FunctionResourceReader::Resource(resource_id) => {
            let (instance, resource_id) = resource_id.consume();

            let mut next_frame_poll_future = {
                let resource_id = resource_id.clone();
                let instance = instance.clone();

                async move {
                    let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
                    instance.future_poll(&resource_id, waker).await.unwrap()
                }.boxed_local()
            };

            match next_frame_poll_future.poll_unpin(cx) {
                Poll::Pending => {
                    return (FunctionResourceReader::FramePollFuture {
                        instance,
                        resource_id,
                        future: next_frame_poll_future,
                    }, Poll::Pending);
                },
                Poll::Ready(Poll::Pending) => {
                    return (FunctionResourceReader::Resource(OwnedFunctionResourceId::new(instance, resource_id)), Poll::Pending);
                },
                Poll::Ready(Poll::Ready(_)) => FunctionResourceReader::FrameReadFuture {
                    instance: instance.clone(),
                    resource_id: resource_id.clone(),
                    future: async move {
                        instance.stream_frame_read(&resource_id).await
                    }.boxed_local(),
                },
            }
        },
        other => other,
    };

    // if there is a frame we are currently reading, poll that
    match reader {
        FunctionResourceReader::FrameReadFuture { instance, resource_id, mut future } => match future.poll_unpin(cx) {
            Poll::Pending => todo!(),
            Poll::Ready(None) => (FunctionResourceReader::Empty, Poll::Ready(None)),
            Poll::Ready(Some(HttpStreamFrame::Bytes(bytes))) => {
                (FunctionResourceReader::Resource(OwnedFunctionResourceId::new(instance, resource_id)), Poll::Ready(Some(Ok(Bytes::from(bytes)))))
            },
            Poll::Ready(Some(HttpStreamFrame::Stream(mut stream))) => {
                match stream.poll_next_unpin(cx) {
                    Poll::Pending => (FunctionResourceReader::Stream(stream), Poll::Pending),
                    Poll::Ready(None) => (FunctionResourceReader::Empty, Poll::Ready(None)),
                    Poll::Ready(Some(frame)) => (FunctionResourceReader::Stream(stream), Poll::Ready(Some(Ok((frame.unwrap()))))),
                }
            },
        },
        _ => unreachable!("didn't expect to get this reader state"),
    }
}
