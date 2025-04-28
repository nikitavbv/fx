# sqlx

- Pool cannot be used because [it calls Instant::now when connecting](https://github.com/launchbadge/sqlx/blob/91d26bad4d5e2b05fab1c86d0fbe11586d30f29d/sqlx-core/src/pool/options.rs#L542).
- compile-time checked queries are not supported yet. this seems to require custom macros implementation, similar to [what authors of sqlx-d1 did](https://github.com/ohkami-rs/sqlx-d1/tree/main/sqlx-d1-macros).
- `sqlx::migrate` macros is also not supported for same reasons as above, it also seems to enable `libsqlite3-sys` dependency, so it would not compile to wasm anyway.
