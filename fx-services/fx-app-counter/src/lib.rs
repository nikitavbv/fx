use {
    std::sync::atomic::{AtomicI64, Ordering},
    fx::{rpc, Result},
};

static COUNTER: AtomicI64 = AtomicI64::new(0);

#[rpc]
pub fn incr() -> Result<i64> {
    Ok(COUNTER.fetch_add(1, Ordering::SeqCst))
}
