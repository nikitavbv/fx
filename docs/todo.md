# todo

- add missing permissions checks
- handle errors everywhere
- rename "storage" to "kv"
- rename "fx-core" to "fx-common"
- split `fx-runtime` into `fx-runtime` and `fx-server` (implementation of triggers and side-effects)
- `fx-common` should contain types for rpc only, no logic. Actual logic (e.g, functions to construct http requests) should live in `fx`.
- remove all `fx-services`, keep `fx-test-app` (rename it to `fx-demo-app`).
- add back queues api
- reload config files.
- better syntax for sql queries.
