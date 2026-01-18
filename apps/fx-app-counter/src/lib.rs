use {
    std::sync::atomic::{AtomicI64, Ordering},
    fx_sdk::{handler, Result},
};

static COUNTER: AtomicI64 = AtomicI64::new(0);

#[handler]
pub async fn incr() -> Result<i64> {
    Ok(COUNTER.fetch_add(1, Ordering::SeqCst))
}
