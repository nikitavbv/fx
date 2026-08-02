use {
    futures::FutureExt,
    crate::{
        sys::{RESOURCE_SET, FunctionResponseFutureResourceKey},
        handler_fn::IntoFunctionResponse,
    },
};

pub fn wrap_function_response_future<T: IntoFunctionResponse>(future: impl Future<Output = T> + 'static) -> FunctionResponseFutureResourceKey {
    let future = future.map(|v| v.into_function_response()).boxed_local();
    RESOURCE_SET.with_borrow_mut(|resources| resources.function_response_futures.insert(future))
}
