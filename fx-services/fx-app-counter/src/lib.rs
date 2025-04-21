use {
    std::sync::atomic::{AtomicI64, Ordering},
    fx::{FxCtx, rpc},
};

static COUNTER: AtomicI64 = AtomicI64::new(0);

#[rpc]
pub fn incr(_ctx: &FxCtx, _req: ()) -> i64 {
    COUNTER.fetch_add(1, Ordering::SeqCst)
}
