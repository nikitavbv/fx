use {
    std::{pin::Pin, future::Future, collections::HashMap},
    thiserror::Error,
    serde::{de::DeserializeOwned, Serialize},
    lazy_static::lazy_static,
    crate::fx_futures::FunctionFutureError,
};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type HandlerFunction = Box<dyn Fn(Vec<u8>) -> HandlerResult + Send + Sync>;

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
    pub static ref HANDLERS: HashMap<&'static str, HandlerFunction> = collect_handlers();
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

pub trait IntoHandler<Args>: Sized + Copy + Send + Sync + 'static {
    fn call(&self, args: Vec<u8>) -> HandlerResult;
    fn into_boxed(self) -> HandlerFunction {
        Box::new(move |args| self.call(args))
    }
}

impl<F, Fut, R> IntoHandler<()> for F
where
    F: Fn() -> Fut + Copy + Send + Sync + 'static,
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
    F: Fn(T1) -> Fut + Copy + Send + Sync + 'static,
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
    F: Fn(T1, T2) -> Fut + Copy + Send + Sync + 'static,
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
