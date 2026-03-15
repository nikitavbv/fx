use {
    futures::FutureExt,
    crate::sys::{FunctionResource, add_function_resource, fx_tasks_background_spawn},
};

/// runs a task in background after request processing is finished
pub fn run_in_background<F>(future: F) where F: Future<Output = ()> + 'static {
    let resource_id = add_function_resource(FunctionResource::BackgroundTask(future.boxed_local())).as_u64();
    unsafe { fx_tasks_background_spawn(resource_id) };
}
