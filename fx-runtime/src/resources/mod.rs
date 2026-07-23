pub(crate) use self::resource::{FunctionResourceId, FunctionResources};

// TODO:
// - there should probably be separate submodules for "host" and "function" resources

pub(crate) mod future;
pub(crate) mod resource;
