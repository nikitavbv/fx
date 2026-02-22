pub(crate) use self::resource::{ResourceId, Resource, FunctionResourceId};

// TODO:
// - there should probably be separate submodules for "host" and "function" resources

pub(crate) mod future;
pub(crate) mod resource;
pub(crate) mod serialize;
