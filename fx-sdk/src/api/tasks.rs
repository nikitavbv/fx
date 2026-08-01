use {
    futures::FutureExt,
    crate::sys::{RESOURCE_SET, fx_tasks_background_spawn},
};

/// runs a task in background after request processing is finished
pub fn run_in_background<F>(future: F) where F: Future<Output = ()> + 'static {
    let resource_id = RESOURCE_SET.with_borrow_mut(|v| v.background_tasks.insert(future.boxed_local()));
    unsafe { fx_tasks_background_spawn(resource_id.into()) };
}
