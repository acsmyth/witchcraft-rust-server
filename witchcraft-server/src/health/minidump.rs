use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use super::{HealthCheck, HealthCheckResult, HealthState};

/// A health check which reports an error state if minidump initialization has not completed successfully.
pub struct MinidumpHealthCheck {
    minidump_ok: Arc<AtomicBool>,
}

impl MinidumpHealthCheck {
    pub fn new(minidump_ok: Arc<AtomicBool>) -> Self {
        MinidumpHealthCheck { minidump_ok }
    }
}

impl HealthCheck for MinidumpHealthCheck {
    fn type_(&self) -> &str {
        "MINIDUMP"
    }

    fn result(&self) -> HealthCheckResult {
        let state = if self.minidump_ok.load(Ordering::Relaxed) {
            HealthState::Healthy
        } else {
            HealthState::Error
        };

        HealthCheckResult::builder().state(state).build()
    }
}
