use {
    tracing::error,
    tikv_jemalloc_ctl::{epoch, stats},
};

pub struct MemoryTracker {
    started_at: Option<u64>,
}

impl MemoryTracker {
    pub fn report_total(&self) -> Option<u64> {
        Some(current_memory_usage()? - self.started_at?)
    }
}

pub fn init_memory_tracker() -> MemoryTracker {
    MemoryTracker { started_at: current_memory_usage() }
}

pub fn current_memory_usage() -> Option<u64> {
    if let Err(err) = epoch::advance() {
        error!("failed to advance jemalloc_ctl epoch: {err:?}");
    }

    match stats::resident::read() {
        Ok(v) => Some(v as u64),
        Err(err) => {
            error!("failed to read stats::resident: {err:?}");
            return None;
        }
    }
}
