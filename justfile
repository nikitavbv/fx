run:
    cargo run -p fx-cloud

app-counter:
    cargo build --target wasm32-unknown-unknown -p fx-app-counter --release

app-hello-world:
    cargo build --target wasm32-unknown-unknown -p fx-app-hello-world --release

app-rpc-test-service:
    cargo build --target wasm32-unknown-unknown -p fx-app-rpc-test-service --release
