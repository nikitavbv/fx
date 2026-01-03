install:
    cargo install --path fx-cloud

run: cloud-dashboard app-counter app-hello-world app-rpc-test-service
    cargo run -p fx-cloud

test:
    cargo build --target wasm32-unknown-unknown -p fx-test-app --release
    cargo test -p fx-runtime --release

coverage:
    cargo llvm-cov --html run -p fx-tests --release

apps: cloud-dashboard app-counter app-hello-world app-rpc-test-service

cloud-dashboard:
    cargo build --target wasm32-unknown-unknown -p fx-cloud-dashboard --release
    cp target/wasm32-unknown-unknown/release/fx_cloud_dashboard.wasm local/functions/dashboard.wasm

app-counter:
    cargo build --target wasm32-unknown-unknown -p fx-app-counter --release

app-hello-world:
    cargo build --target wasm32-unknown-unknown -p fx-app-hello-world --release

app-rpc-test-service:
    cargo build --target wasm32-unknown-unknown -p fx-app-rpc-test-service --release

local-http: cloud-dashboard
    cargo run -p fx-cloud --release -- --functions-dir local/functions http dashboard

local-cron: app-hello-world
    cp target/wasm32-unknown-unknown/release/fx_app_hello_world.wasm local/functions/hello-world.wasm
    cargo run -p fx-cloud --release -- --functions-dir local/functions cron local/cron.yaml
