# sqlx

Pool cannot be used because [it calls Instant::now when connecting](https://github.com/launchbadge/sqlx/blob/91d26bad4d5e2b05fab1c86d0fbe11586d30f29d/sqlx-core/src/pool/options.rs#L542).
