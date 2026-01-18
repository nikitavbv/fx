fn main() {
    ::capnpc::CompilerCommand::new()
        .file("fx.capnp")
        .run()
        .expect("compiling schema");
}
