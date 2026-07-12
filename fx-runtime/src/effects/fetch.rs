use {
    hyper::body::Bytes,
    thiserror::Error,
    crate::{
        function::abi::{capnp, abi_http_capnp},
        resources::{
            serialize::DeserializeFunctionResource,
            resource::{OwnedFunctionResourceId, HttpBodyResourceKey, FunctionResources},
            FunctionResourceId,
        },
        triggers::http::HttpBody,
        tasks::worker::FunctionInvokeError,
    },
};

#[derive(Error, Debug)]
pub(crate) enum FetchResultError {
    #[error("connection failed")]
    ConnectionFailed,
    #[error("connection timeout")]
    ConnectionTimeout,
    #[error("response timeout")]
    ResponseTimeout,

    #[error("rpc target function not found")]
    FunctionNotFound,
    #[error("rpc target function panicked")]
    FunctionPanicked,
    #[error("rpc target function is busy handling other requests and cannot accept a new one")]
    FunctionBusy,
    #[error("runtime is being shut down, new requests are not accepted")]
    RuntimeShutdown,
}

impl From<FunctionInvokeError> for FetchResultError {
    fn from(err: FunctionInvokeError) -> Self {
        match err {
            FunctionInvokeError::NotFound => Self::FunctionNotFound,
            FunctionInvokeError::FunctionPanicked => Self::FunctionPanicked,
            FunctionInvokeError::FunctionBusy => Self::FunctionBusy,
        }
    }
}

impl From<crate::tasks::worker::local_worker_controller::invoke_function::FunctionInvokeError> for FetchResultError {
    fn from(err: crate::tasks::worker::local_worker_controller::invoke_function::FunctionInvokeError) -> Self {
        use crate::tasks::worker::local_worker_controller::invoke_function::FunctionInvokeError as SourceError;
        match err {
            SourceError::NotFound => Self::FunctionNotFound,
            SourceError::FunctionPanicked => Self::FunctionPanicked,
            SourceError::FunctionBusy => Self::FunctionBusy,
            SourceError::RuntimeShutdown => Self::RuntimeShutdown,
        }
    }
}

pub(crate) struct FetchResultWithBodyResource {
    pub(crate) parts: http::response::Parts,
    pub(crate) body: HttpBodyResourceKey,
}

impl FetchResultWithBodyResource {
    pub fn new(parts: http::response::Parts, body: HttpBodyResourceKey) -> Self {
        Self { parts, body }
    }
}

#[derive(Error, Debug)]
pub enum HttpStreamError {
    #[error("failed to read fetch response stream")]
    FetchResponseStreamError(reqwest::Error),
    #[error("failed to read http request body stream")]
    RequestBodyStreamError,
}

impl DeserializeFunctionResource for HttpBody {
    type Error = HttpBodyDeserializeError;

    fn deserialize(_function_resources: &mut FunctionResources, resource: &mut &[u8], instance: std::rc::Rc<crate::function::instance::FunctionInstance>) -> Result<Self, Self::Error> {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let http_body = message_reader.get_root::<abi_http_capnp::http_body::Reader>().unwrap();

        Ok(match http_body.get_body().which().unwrap() {
            abi_http_capnp::http_body::body::Which::Empty(_) => todo!(),
            abi_http_capnp::http_body::body::Which::Bytes(v) => Self::for_bytes(Bytes::copy_from_slice(v.unwrap())),
            abi_http_capnp::http_body::body::Which::FunctionStream(v) => Self::for_function_stream(OwnedFunctionResourceId::new(instance, FunctionResourceId::new(v))),
            abi_http_capnp::http_body::body::Which::HostResource(v) => instance.store.try_lock().unwrap().data_mut().resource_set.http_bodies.remove(v.into()).unwrap(),
        })
    }
}

#[derive(Debug, Error)]
pub(crate) enum HttpBodyDeserializeError {
}
