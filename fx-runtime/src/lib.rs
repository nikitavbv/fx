// target code structure:
// function/ - functions, deployments, function state
// function/abi.rs - handlers for wasm exports/imports
// resource/ - resource table and everything related to it
// trigger/{http, cron} - what can call function
// effect/{sql, blob, fetch, sleep, time,...} - implementations of effects
// introspection/ - management UI
// tasks/{messages, worker, sql, managament, compiler} - each group of threads is "task"
// definitions/ - configs and definitions live here
// permissions/
// lib.rs - export types for integration tests
// main.rs - construct and start server

// TODO:
// - rate limiting - use governor crate and have a set of rate limits defined in FunctionDefinition
// - permissions - based on capabilities

pub use crate::{server::FxServer, definitions::config};

mod definitions;
mod effects;
mod function;
mod introspection;
mod resources;
mod server;
mod tasks;
mod triggers;
