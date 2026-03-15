use {
    futures::FutureExt,
    crate::sys::FunctionResource,
};

/// runs a task in background after request processing is finished
pub fn run_in_background<F>(future: F) where F: Future<Output = ()> + 'static {
    let task_resource = FunctionResource::BackgroundTask(future.boxed_local());
    todo!()
}
