use {
    std::{io, time::Duration},
    crate::effects::metrics::{MetricKey, MetricsRegistry},
};

pub(super) const COLLECT_INTERVAL: Duration = Duration::from_secs(10);

pub(super) struct SystemMetricsCollector;

impl SystemMetricsCollector {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) async fn collect(&self, metrics: &MetricsRegistry) {
        self.collect_fd_usage(metrics).await;
    }

    async fn collect_fd_usage(&self, metrics: &MetricsRegistry) {
        let limits = nofile_limits();
        metrics.gauge_update(MetricKey::new("fx_nofile_soft_limit"), limits.soft as i64);
        metrics.gauge_update(MetricKey::new("fx_nofile_hard_limit"), limits.hard as i64);

        match sample_open_fds().await {
            Ok(open_fds) => {
                metrics.gauge_update(MetricKey::new("fx_open_fds"), open_fds as i64);
            },
            Err(err) => {
                metrics.counter_increment(MetricKey::new("fx_fd_sample_errors_total").with_label("reason", err.label()), 1);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FdSampleError {
    AtLimit,
    Unavailable,
}

impl FdSampleError {
    fn label(self) -> &'static str {
        match self {
            Self::AtLimit => "at_limit",
            Self::Unavailable => "unavailable",
        }
    }
}

struct NoFileLimits {
    soft: u64,
    hard: u64,
}

async fn sample_open_fds() -> Result<u64, FdSampleError> {
    let mut directory = match tokio::fs::read_dir("/proc/self/fd").await {
        Ok(v) => v,
        Err(_) => tokio::fs::read_dir("/dev/fd").await.map_err(fd_sample_error)?,
    };

    let mut count = 0u64;

    loop {
        match directory.next_entry().await {
            Ok(Some(_)) => count += 1,
            Ok(None) => break,
            Err(err) => return Err(fd_sample_error(err)),
        }
    }

    Ok(count.saturating_sub(1))
}

fn fd_sample_error(err: io::Error) -> FdSampleError {
    if matches!(err.raw_os_error(), Some(libc::EMFILE | libc::ENFILE)) {
        FdSampleError::AtLimit
    } else {
        FdSampleError::Unavailable
    }
}

fn nofile_limits() -> NoFileLimits {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();

    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
        return NoFileLimits { soft: 0, hard: 0 };
    }

    let limit = unsafe { limit.assume_init() };

    NoFileLimits {
        soft: limit.rlim_cur,
        hard: limit.rlim_max,
    }
}
