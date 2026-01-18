# fx

building something similar to [workerd](https://github.com/cloudflare/workerd) for fun and learning purposes.

## features

- deploy functions written in Rust (see `fx-sdk` for sdk and `apps` for examples) compiled to wasm (`wasm32-unknown-unknown`).
- functions can be triggered by http requests, messages from Kafka topics or by cron schedule.
- functions are async and can handle requests concurrently.
- function input and output can be a stream.
- KV storage.
- sql databases powered by sqlite.
- functions can call other functions via RPC (callee is run in the same thread, so no networking is involved resulting in very low latency).
- `log`, `fetch`, `random`, `time` apis.
- livereload functions.

still, note that this is a toy project with a lot of apis missing. it is also missing handling for various edge cases/errors.

## example function

```rust
use { fx_sdk::{self as fx, handler, HttpRequest, HttpResponse}, tracing::info };

#[handler]
pub async fn http(req: HttpRequest) -> fx::Result<HttpResponse> {
    info!("this function is compiled to wasm and is running within fx: {}", req.url);
    Ok(HttpResponse::new().body("hello fx!\n"))
}
```

see [apps](./apps) for more examples.

## performance

fx performs on par with Node.js and at 46% of native Rust.

| Runtime | Requests/sec | Latency |
|---------|-------------|---------|
| fx | 17,499 | 455μs |
| axum (native Rust) | 37,963 | 221μs |
| Node.js | 17,200 | 465μs |

Benchmark: TechEmpower Fortunes on Hetzner CCX23. See [docs/benchmarking.md](docs/benchmarking.md) for details.

## usage

### invoke functions using cli interface

build hello world example (alternatively, compile your own function to `.wasm`):
```bash
just app-hello-world
```

then run function using fx:
```bash
cargo run -p fx-runtime -- run target/wasm32-unknown-unknown/release/fx_app_hello_world example
```

expected output:
```
target/wasm32-unknown-unknown/release/fx_app_hello_world | {"message": Text("hello from fx!")}
```
