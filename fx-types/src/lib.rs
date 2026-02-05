pub use capnp;

pub mod abi;

capnp::generated_code!(pub mod abi_capnp);
capnp::generated_code!(pub mod abi_function_resources_capnp);
capnp::generated_code!(pub mod abi_log_capnp);
capnp::generated_code!(pub mod events_capnp);
