pub use {
    fx_macro::fetch,
    ctor::ctor,
};

use {
    std::{pin::Pin, future::Future, collections::HashMap, sync::Mutex, sync::OnceLock},
    thiserror::Error,
    serde::{de::DeserializeOwned, Serialize},
    lazy_static::lazy_static,
    futures::FutureExt,
    crate::fx_futures::FunctionFutureError,
};

// v2 handlers:
pub(crate) static HANDLER_FETCH: OnceLock<HttpHandlerFunction> = OnceLock::new();
type HttpHandlerFunction = Box<dyn Fn(FunctionRequest) -> BoxFuture<FunctionResponse> + Send + Sync>;

pub struct FunctionRequest {}

impl FunctionRequest {
    pub fn into_legacy_http_request(self) -> crate::HttpRequest {
        crate::HttpRequest::new()
    }
}

pub struct FunctionResponse {
}

impl FunctionResponse {
    pub fn from_legacy_http_response(response: crate::HttpResponse) -> Self {
        Self {
        }
    }

    pub fn into_legacy_http_response(self) -> crate::HttpResponse {
        crate::HttpResponse::new()
    }
}

pub trait IntoFunctionResult {}

pub fn register_http_fetch_handler<F, Fut>(f: F)
where
    F: Fn(crate::HttpRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = crate::Result<fx_common::HttpResponse>> + 'static + Send {
    let _ = HANDLER_FETCH.set(Box::new(move |req| {
        Box::pin(f(req.into_legacy_http_request()).map(|v| FunctionResponse::from_legacy_http_response(v.unwrap()))) as BoxFuture<FunctionResponse>
    }));
}

// v1 handlers:
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type HandlerFunction = Box<dyn Fn(Vec<u8>) -> HandlerResult + Send>;

// error type for user application errors + surrounding infrastructure errors
#[derive(Error, Debug)]
pub enum HandlerError {
    /// error produced by user's implementation of the handler
    #[error("user application error: {description}")]
    UserApplicationError {
        description: String,
    },

    /// Failed to deserialize function argument
    #[error("failed to deserialize function argument: {reason:?}")]
    ArgumentDeserializationError {
        reason: String,
    },
}

impl From<anyhow::Error> for HandlerError {
    fn from(err: anyhow::Error) -> Self {
        Self::UserApplicationError { description: format!("{err:?}") }
    }
}

/// when function future is pushed into futures arena, its error type
/// should be converted into a more generic FxFutureError, that combines
/// all cases where a function future can fail.
impl From<HandlerError> for FunctionFutureError {
    fn from(value: HandlerError) -> Self {
        match value {
            HandlerError::UserApplicationError { description } => Self::UserApplicationError { description },
            HandlerError::ArgumentDeserializationError { reason } => Self::FunctionArgumentDeserializationError { reason },
        }
    }
}

// result type for user's implementation of the handler + runtime infrastructure like serialization, etc.
pub type HandlerResult = BoxFuture<Result<Vec<u8>, HandlerError>>;

// result type for user's implementation of the handler
type UserHandlerResult<T: Serialize + 'static> = anyhow::Result<T>;

lazy_static! {
    pub static ref HANDLERS: Mutex<HashMap<&'static str, HandlerFunction>> = Mutex::new(collect_handlers());
}

fn collect_handlers() -> HashMap<&'static str, HandlerFunction> {
    let mut handlers = HashMap::new();

    for handler in inventory::iter::<Handler>() {
        handlers.insert(handler.name, (handler.make_handler)());
    }

    handlers
}

pub struct Handler {
    name: &'static str,
    make_handler: fn() -> HandlerFunction,
}
inventory::collect!(Handler);

impl Handler {
    pub const fn new(name: &'static str, make_handler: fn() -> HandlerFunction) -> Self {
        Self {
            name,
            make_handler,
        }
    }

    pub fn invoke(&self, args: Vec<u8>) -> HandlerResult {
        (self.make_handler)()(args)
    }
}

pub trait IntoHandler<Args>: Sized + Copy + Send + 'static {
    fn call(&self, args: Vec<u8>) -> HandlerResult;
    fn into_boxed(self) -> HandlerFunction {
        Box::new(move |args| self.call(args))
    }
}

impl<F, Fut, R> IntoHandler<()> for F
where
    F: Fn() -> Fut + Copy + Send + 'static,
    Fut: Future<Output = UserHandlerResult<R>> + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, _args: Vec<u8>) -> HandlerResult {
        let future = self();
        Box::pin(async move {
            let result = future.await?;
            serialize(result)
        })
    }
}

impl<F, Fut, T1, R> IntoHandler<(T1,)> for F
where
    F: Fn(T1) -> Fut + Copy + Send + 'static,
    Fut: Future<Output = UserHandlerResult<R>> + Send + 'static,
    T1: DeserializeOwned + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, args: Vec<u8>) -> HandlerResult {
        let f = *self;
        let arg: Result<T1, HandlerError> = deserialize(args);
        Box::pin(async move {
            let result = f(arg?).await?;
            serialize(result)
        })
    }
}

impl<F, Fut, T1, T2, R> IntoHandler<(T1, T2)> for F
where
    F: Fn(T1, T2) -> Fut + Copy + Send + 'static,
    Fut: Future<Output = UserHandlerResult<R>> + Send + 'static,
    T1: DeserializeOwned + Send + 'static,
    T2: DeserializeOwned + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, args: Vec<u8>) -> HandlerResult {
        let f = *self;
        let args: Result<(T1, T2), HandlerError> = deserialize(args);
        Box::pin(async move {
            let (arg1, arg2) = args?;
            let result = f(arg1, arg2).await?;
            serialize(result)
        })
    }
}

pub fn serialize<T: Serialize>(data: T) -> Result<Vec<u8>, HandlerError> {
    Ok(rmp_serde::to_vec(&data).unwrap())
}

pub fn deserialize<T: DeserializeOwned>(data: Vec<u8>) -> Result<T, HandlerError> {
    rmp_serde::from_slice(&data)
        .map_err(|err| HandlerError::ArgumentDeserializationError {
            reason: err.to_string(),
        })
}
