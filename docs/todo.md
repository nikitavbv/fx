# todo

- add missing permissions checks
- handle errors everywhere
- rename "storage" to "kv"
- rename "fx-core" to "fx-common"
- split `fx-cloud` into `fx-core` (runtime) and `fx-runtime` (implementation of side-effects)
- `fx-common` should contain types for rpc only, no logic. Actual logic (e.g, functions to construct http requests) should live in `fx`.
- remove all `fx-services`, keep `fx-test-app` (rename it to `fx-demo-app`).
- add back queues api
- rename "ServiceId" to "FunctionId"
- reload config files.
- better syntax for sql queries.
