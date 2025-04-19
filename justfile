run:
    cargo run -p fx-cloud

app-hello-world:
    cargo build --target wasm32-unknown-unknown -p fx-app-hello-world --release
