use {
    futures::FutureExt,
    crate::{
        sys::{FunctionResource, FunctionResourceId, add_function_resource},
        handler::{FunctionResponse, IntoFunctionResponse},
    },
};

pub fn wrap_function_response_future<T: IntoFunctionResponse>(future: impl Future<Output = T> + 'static) -> FunctionResourceId {
    let future = future.map(|v| v.into_function_response()).boxed_local();
    let future = FunctionResource::FunctionResponseFuture(future);
    add_function_resource(future)
}
