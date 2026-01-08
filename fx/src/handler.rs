use {
    std::{pin::Pin, future::Future},
    serde::{de::DeserializeOwned, Serialize},
    crate::error::FxError,
};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub struct Handler {
    pub name: &'static str,
    pub make_handler: fn() -> Box<dyn Fn(Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> + Send + Sync>,
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
    Fut: Future<Output = R> + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, _args: Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> {
        let future = self();
        Box::pin(async move {
            let result = future.await;
            serialize(result)
        })
    }
}

impl<F, Fut, T1, R> IntoHandler<(T1,)> for F
where
    F: Fn(T1) -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    T1: DeserializeOwned + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, args: Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> {
        let f = *self;
        let arg: Result<T1, FxError> = deserialize(args);
        Box::pin(async move {
            let result = f(arg?).await;
            serialize(result)
        })
    }
}

impl<F, Fut, T1, T2, R> IntoHandler<(T1, T2)> for F
where
    F: Fn(T1, T2) -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    T1: DeserializeOwned + Send + 'static,
    T2: DeserializeOwned + Send + 'static,
    R: Serialize + 'static,
{
    fn call(&self, args: Vec<u8>) -> BoxFuture<Result<Vec<u8>, FxError>> {
        let f = *self;
        let args: Result<(T1, T2), FxError> = deserialize(args);
        Box::pin(async move {
            let (arg1, arg2) = args?;
            let result = f(arg1, arg2).await;
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

async fn test_function(arg: u32) -> u32 {
    arg + 1
}

inventory::submit! {
    Handler {
        name: "test_function",
        make_handler: || { IntoHandler::into_boxed(test_function) }
    }
}
