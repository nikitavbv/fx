use {
    std::{pin::Pin, future::Future, collections::HashMap},
    serde::{de::DeserializeOwned, Serialize},
    lazy_static::lazy_static,
    crate::error::FxError,
};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type HandlerFunction = Box<dyn Fn(Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> + Send + Sync>;

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
    pub name: &'static str,
    pub make_handler: fn() -> HandlerFunction,
}
inventory::collect!(Handler);

impl Handler {
    pub fn invoke(&self, args: Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> {
        (self.make_handler)()(args)
    }
}

pub trait IntoHandler<Args>: Sized + Copy + Send + Sync + 'static {
    fn call(&self, args: Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>>;
    fn into_boxed(self) -> Box<dyn Fn(Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> + Send + Sync> {
        Box::new(move |args| self.call(args))
    }
}

impl<F, Fut, R> IntoHandler<()> for F
where
    F: Fn() -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = Result<R, FxError>> + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, _args: Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> {
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
    Fut: Future<Output = Result<R, FxError>> + Send + 'static,
    T1: DeserializeOwned + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, args: Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> {
        let f = *self;
        let arg: Result<T1, FxError> = deserialize(args);
        Box::pin(async move {
            let result = f(arg?).await?;
            serialize(result)
        })
    }
}

impl<F, Fut, T1, T2, R> IntoHandler<(T1, T2)> for F
where
    F: Fn(T1, T2) -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = Result<R, FxError>> + Send + 'static,
    T1: DeserializeOwned + Send + 'static,
    T2: DeserializeOwned + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, args: Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> {
        let f = *self;
        let args: Result<(T1, T2), FxError> = deserialize(args);
        Box::pin(async move {
            let (arg1, arg2) = args?;
            let result = f(arg1, arg2).await?;
            serialize(result)
        })
    }
}

pub fn serialize<T: Serialize>(data: T) -> Result<Vec<u8>, FxError> {
    Ok(rmp_serde::to_vec(&data).unwrap())
}

pub fn deserialize<T: DeserializeOwned>(data: Vec<u8>) -> Result<T, FxError> {
    Ok(rmp_serde::from_slice(&data).unwrap())
}

async fn test_function(arg: u32) -> Result<u32, FxError> {
    Ok(arg + 1)
}

inventory::submit! {
    Handler {
        name: "test_function",
        make_handler: || { IntoHandler::into_boxed(test_function) }
    }
}
