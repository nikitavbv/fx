# fx

building something similar to [workerd](https://github.com/cloudflare/workerd) for fun and learning purposes.

## features

- deploy functions written in Rust (see `fx` for sdk and `fx-services` for examples) compiled to wasm (`wasm32-unknown-unknown`).
- functions can be triggered by http requests, messages from Kafka topics or by cron schedule.
- functions are async and can handle requests concurrently.
- function input and output can be a stream.
- KV storage.
- sql databases powered by sqlite.
- functions can call other functions via RPC (callee is run in the same thread, so no networking is involved resulting in very low latency).
- `log`, `fetch`, `random`, `time` apis.
- livereload functions.

still, note that this is a toy project with a lot of apis missing. it is also missing handling for various edge cases/errors. do not expect it to handle any significant rps as well at this moment.

## example function

```rust
use { fx::{FxCtx, HttpRequest, HttpResponse, rpc}, tracing::info };

#[rpc]
pub async fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
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
