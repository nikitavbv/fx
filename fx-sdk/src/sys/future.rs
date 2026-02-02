use {
    std::sync::Mutex,
    futures::FutureExt,
    crate::{
        sys::{FunctionResource, FunctionResourceId, add_function_resource},
        handler::FunctionResponse,
    },
};

pub fn wrap_function_response_future(future: impl Future<Output = FunctionResponse> + Send + 'static) -> FunctionResourceId {
    let future = future.boxed();
    let future = FunctionResource::FunctionResponseFuture(Mutex::new(future));
    add_function_resource(future)
}
