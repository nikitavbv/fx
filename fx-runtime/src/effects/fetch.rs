use {
    hyper::body::Bytes,
    thiserror::Error,
    crate::{
        function::abi::{capnp, abi_http_capnp},
        resources::{
            serialize::{SerializeResource, SerializableResource, DeserializeFunctionResource},
            ResourceId,
            resource::{OwnedFunctionResourceId, Resource},
            FunctionResourceId,
        },
        triggers::http::HttpBody,
    },
};

pub(crate) enum FetchResult {
    Inline(FetchResultInline),
    BodyResource(SerializableResource<Result<FetchResultWithBodyResource, FetchResultError>>),
}

impl FetchResult {
    pub fn new(parts: http::response::Parts, body: HttpBody) -> Self {
        Self::Inline(FetchResultInline::new(parts, body))
    }

    pub fn error(err: FetchResultError) -> Self {
        Self::BodyResource(SerializableResource::Raw(Err(err)))
    }
}

#[derive(Error, Debug)]
pub(crate) enum FetchResultError {
    #[error("connection failed")]
    ConnectionFailed,
    #[error("connection timeout")]
    ConnectionTimeout,
    #[error("response timeout")]
    ResponseTimeout,
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

impl SerializeResource for Result<FetchResultWithBodyResource, FetchResultError> {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let fetch_result = message.init_root::<abi_http_capnp::fetch_result::Builder>();

        match self {
            Ok(ok) => {
                let mut ok_builder = fetch_result.init_result().init_ok();
                ok_builder.set_status(ok.parts.status.as_u16());
                let mut headers = ok_builder.reborrow().init_headers(ok.parts.headers.len() as u32);
                for (index, (name, value)) in ok.parts.headers.iter().enumerate() {
                    let mut header = headers.reborrow().get(index as u32);
                    header.set_name(name.as_str());
                    header.set_value(value.to_str().unwrap());
                }
                ok_builder.reborrow().set_body_resource_id(ok.body.as_u64());
            }
            Err(err) => {
                let mut error_builder = fetch_result.init_result().init_error().init_error();
                match err {
                    FetchResultError::ConnectionFailed => error_builder.set_connection_failed(()),
                    FetchResultError::ConnectionTimeout => error_builder.set_connection_timeout(()),
                    FetchResultError::ResponseTimeout => error_builder.set_response_timeout(()),
                }
            }
        }

        capnp::serialize::write_message_to_words(&message)
    }
}

#[derive(Error, Debug)]
pub enum HttpStreamError {
    #[error("failed to read fetch response stream")]
    FetchResponseStreamError(reqwest::Error),
}

impl DeserializeFunctionResource for HttpBody {
    fn deserialize(resource: &mut &[u8], instance: std::rc::Rc<crate::function::instance::FunctionInstance>) -> Self {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let http_body = message_reader.get_root::<abi_http_capnp::http_body::Reader>().unwrap();

        match http_body.get_body().which().unwrap() {
            abi_http_capnp::http_body::body::Which::Empty(_) => todo!(),
            abi_http_capnp::http_body::body::Which::Bytes(v) => Self::for_bytes(Bytes::copy_from_slice(v.unwrap())),
            abi_http_capnp::http_body::body::Which::FunctionStream(v) => Self::for_function_stream(OwnedFunctionResourceId::new(instance, FunctionResourceId::new(v))),
            abi_http_capnp::http_body::body::Which::HostResource(v) => {
                let resource_id = ResourceId::new(v);
                let body = instance.store.try_lock().unwrap().data_mut().resource_remove(&resource_id);
                match body {
                    Resource::HttpBody(v) => v,
                    other => todo!(),
                }
            },
        }
    }
}

impl SerializeResource for Option<Result<hyper::body::Bytes, HttpStreamError>> {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let serialized_frame = message.init_root::<abi_http_capnp::http_body_frame::Builder>();
        let mut serialized_frame = serialized_frame.init_frame();

        match self {
            Some(v) => serialized_frame.set_bytes(&v.unwrap().to_vec()),
            None => serialized_frame.set_stream_end(()),
        }

        capnp::serialize::write_message_to_words(&message)
    }
}
