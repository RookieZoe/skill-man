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
}
