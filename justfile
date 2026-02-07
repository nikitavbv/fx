install:
    cargo install --path fx-cloud

run:
    cargo run -p fx-runtime -- serve local/fx.yaml

test:
    cargo build --target wasm32-unknown-unknown -p fx-test-app --release
    cargo test -p fx-runtime --release

coverage:
    cargo llvm-cov --html run -p fx-tests --release

demo: app-demo run

app-demo: app-fortunes
    cargo build --target wasm32-unknown-unknown -p fx-app-demo --release
    cargo build --target wasm32-unknown-unknown -p fx-fortunes --release

    cp target/wasm32-unknown-unknown/release/fx_app_demo.wasm local/functions/demo.wasm
    cp apps/fx-app-demo/demo.fx.yaml local/functions/demo.fx.yaml

    cp target/wasm32-unknown-unknown/release/fx_fortunes.wasm local/functions/fortunes.wasm
    rm local/functions/fortunes.sqlite || true
    sqlite3 local/functions/fortunes.sqlite < apps/fx-fortunes/fortunes.sql
    cp apps/fx-fortunes/fortunes.fx.yaml local/functions/fortunes.fx.yaml

app-counter:
    cargo build --target wasm32-unknown-unknown -p fx-app-counter --release

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

fortunes-baseline-axum: fortunes-env-setup
    cd extra/benchmark-baseline-axum && cargo build --release && cd ../..
    cp extra/benchmark-baseline-axum/target/release/benchmark-baseline-axum local/fortunes/benchmark-baseline-axum
    ./local/fortunes/benchmark-baseline-axum

fortunes-baseline-nodejs: fortunes-env-setup
    cd extra/benchmark-baseline-node && npm install && node server.js

fortunes-benchmark:
    # first, warmup
    wrk -t4 -c10 -d5s http://localhost:8080/fortunes > /dev/null
    # run the benchmark
    wrk -t4 -c10 -d60s http://localhost:8080/fortunes
