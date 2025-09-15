# todo

- handle errors everywhere
- rename "storage" to "kv"
- rename "fx-core" to "fx-common"
- `fx-common` should contain types for rpc only, no logic. Actual logic (e.g, functions to construct http requests) should live in `fx`.
- remove all `fx-services`, keep `fx-test-app` (rename it to `fx-demo-app`).
- add back queues api
- reload config files.
- better syntax for sql queries.
- create "subsystems" module (where definition provider, kv, sql, metrics, compiler, logs) will go
- compatibility.rs should become api/placeholders.rs
- consumer.rs should go to `fx-server`
- remove unused dependencies in Cargo.toml of `fx-runtime`
- one error per function instead of `FxRuntimeError`.
