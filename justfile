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

app-fortunes:
    cargo build --target wasm32-unknown-unknown -p fx-fortunes --release

fortunes-env-setup: app-fortunes
    mkdir -p local/fortunes
    rm local/fortunes/fortunes.sqlite || true
    sqlite3 local/fortunes/fortunes.sqlite < apps/fx-fortunes/fortunes.sql
    cp apps/fx-fortunes/fortunes.fx.yaml local/fortunes/fortunes.fx.yaml
    cp target/wasm32-unknown-unknown/release/fx_fortunes.wasm local/fortunes/fortunes.wasm

fortunes-server: fortunes-env-setup
    cargo run -p fx-server --release -- --functions-dir local/fortunes --logger noop http fortunes --port 8080

fortunes-baseline-server: fortunes-env-setup
    cd extra/benchmark-baseline-axum && cargo build --release && cd ../..
    cp extra/benchmark-baseline-axum/target/release/benchmark-baseline-axum local/fortunes/benchmark-baseline-axum
    ./local/fortunes/benchmark-baseline-axum

fortunes-benchmark:
    # first, warmup
    curl -s http://localhost:8080/ > /dev/null
    # run the benchmark
    wrk -t4 -c256 -d15s http://localhost:8080/fortunes

local-http: cloud-dashboard
    cargo run -p fx-cloud --release -- --functions-dir local/functions http dashboard

local-cron: app-hello-world
    cp target/wasm32-unknown-unknown/release/fx_app_hello_world.wasm local/functions/hello-world.wasm
    cargo run -p fx-cloud --release -- --functions-dir local/functions cron local/cron.yaml
