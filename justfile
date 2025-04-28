run:
    cargo run -p fx-cloud

test:
    cargo build --target wasm32-unknown-unknown -p fx-test-app --release
    cargo run -p fx-tests --release

apps: cloud-dashboard app-counter app-hello-world app-rpc-test-service

cloud-dashboard:
    cargo build --target wasm32-unknown-unknown -p fx-cloud-dashboard --release

app-counter:
    cargo build --target wasm32-unknown-unknown -p fx-app-counter --release

app-hello-world:
    cargo build --target wasm32-unknown-unknown -p fx-app-hello-world --release

app-rpc-test-service:
    cargo build --target wasm32-unknown-unknown -p fx-app-rpc-test-service --release
