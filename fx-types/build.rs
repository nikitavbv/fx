fn main() {
    ::capnpc::CompilerCommand::new()
        .file("abi.capnp")
        .file("events.capnp")
        .run()
        .expect("compiling schema");
}
