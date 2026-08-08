use std::sync::atomic::{AtomicBool, Ordering};

pub struct RecoveryGate {
    ready: AtomicBool,
}

impl RecoveryGate {
    pub fn ready() -> Self {
        Self {
            ready: AtomicBool::new(true),
        }
    }

    pub fn blocked() -> Self {
        Self {
            ready: AtomicBool::new(false),
        }
    }

    pub fn writes_are_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    /// Lock writes for the rest of the session (spec §10.4): used when an
    /// operation fails and cannot compensate, leaving recovery to the next
    /// startup.
    pub fn mark_blocked(&self) {
        self.ready.store(false, Ordering::Release);
    }
}
