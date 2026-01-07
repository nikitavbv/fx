use {
    serde::{de::DeserializeOwned, Serialize},
    crate::error::FxError,
};

pub struct Handler {
    pub name: &'static str,
    pub make_handler: fn() -> Box<dyn Fn(Vec<u8>) -> Result<Vec<u8>, FxError> + Send + Sync>,
}
inventory::collect!(Handler);

impl Handler {
    pub fn invoke(&self, args: Vec<u8>) -> Result<Vec<u8>, FxError> {
        (self.make_handler)()(args)
    }
}

pub trait IntoHandler<Args>: Sized + Copy + Send + Sync + 'static {
    fn call(&self, args: Vec<u8>) -> Result<Vec<u8>, FxError>;
    fn into_boxed(self) -> Box<dyn Fn(Vec<u8>) -> Result<Vec<u8>, FxError> + Send + Sync> {
        Box::new(move |args| self.call(args))
    }
}

impl<F, R> IntoHandler<()> for F
where
    F: Fn() -> R + Copy + Send + Sync + 'static,
    R: Serialize,
{
    fn call(&self, _args: Vec<u8>) -> Result<Vec<u8>, FxError> {
        let result = self();
        serialize(result)
    }
}

impl<F, T1, R> IntoHandler<(T1,)> for F
where
    F: Fn(T1) -> R + Copy + Send + Sync + 'static,
    T1: DeserializeOwned,
    R: Serialize,
{
    fn call(&self, args: Vec<u8>) -> Result<Vec<u8>, FxError> {
        let arg: T1 = deserialize(args)?;
        let result = self(arg);
        serialize(result)
    }
}

impl<F, T1, T2, R> IntoHandler<(T1, T2)> for F
where
    F: Fn(T1, T2) -> R + Copy + Send + Sync + 'static,
    T1: DeserializeOwned,
    T2: DeserializeOwned,
    R: Serialize,
{
    fn call(&self, args: Vec<u8>) -> Result<Vec<u8>, FxError> {
        let (arg1, arg2): (T1, T2) = deserialize(args)?;
        let result = self(arg1, arg2);
        serialize(result)
    }
}

pub fn serialize<T: Serialize>(data: T) -> Result<Vec<u8>, FxError> {
    Ok(rmp_serde::to_vec(&data).unwrap())
}

pub fn deserialize<T: DeserializeOwned>(data: Vec<u8>) -> Result<T, FxError> {
    Ok(rmp_serde::from_slice(&data).unwrap())
}

// test:
fn test_function(arg: u32) -> u32 {
    arg + 1
}

inventory::submit! {
    Handler {
        name: "test_function",
        make_handler: || { IntoHandler::into_boxed(test_function) }
    }
}
