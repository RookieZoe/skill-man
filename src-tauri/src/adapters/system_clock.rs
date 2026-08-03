use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::seams::clock::Clock;

pub struct SystemClock {
    started_at: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn monotonic_millis(&self) -> u128 {
        self.started_at.elapsed().as_millis()
    }

    fn unix_epoch_nanos(&self) -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    }
}
