fn main() {
    ::capnpc::CompilerCommand::new()
        .file("abi_blob.capnp")
        .file("abi_http.capnp")
        .file("abi_kv.capnp")
        .file("abi_log.capnp")
        .file("abi_metrics.capnp")
        .file("abi_sql.capnp")
        .file("events.capnp")
        .run()
        .expect("compiling schema");
}
