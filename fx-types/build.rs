fn main() {
    ::capnpc::CompilerCommand::new()
        .file("abi.capnp")
        .file("abi_function_resources.capnp")
        .file("abi_log.capnp")
        .file("events.capnp")
        .run()
        .expect("compiling schema");
}
