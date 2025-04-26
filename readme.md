# fx

building something similar to [workerd](https://github.com/cloudflare/workerd) or [wasmCloud](https://github.com/wasmCloud/wasmCloud) for fun and learning purposes.

## features

- deploy functions written in Rust (see `fx` for sdk and `fx-services` for examples) compiled to wasm (`wasm32-unknown-unknown`).
- functions can be triggered by http requests or messages from Kafka topics.
- KV storage.
- sql databases powered by sqlite accessible from functions.
- functions can call other functions via RPC (callee is run in the same thread, so no networking is involved resulting in very low latency).
- multiple instances of each function spawned, but functions marked as "global" only run in single instance. the latter can be used as a stateful actor or a building block in a distributed system.
- `log`, `fetch` apis.

still, note that this is a toy project with a lot of apis missing. it is also missing handling for various edge cases/errors. do not expect it to handle any significant rps as well at this moment.

## example function

```rust
use { fx::{FxCtx, HttpRequest, HttpResponse, rpc}, tracing::info };

#[rpc]
pub fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();
    info!("this function is compiled to wasm and is running within fx: {}", req.url);
    HttpResponse::new().body("hello fx!\n")
}
```

see [fx-services](./fx-services) for more examples.

## usage

```bash
cargo run -p fx-cloud -- ./app.wasm
```
