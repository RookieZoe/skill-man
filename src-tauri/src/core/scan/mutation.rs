//! Scan mutation generation (spec §4.10, ADR-0020): "相关产品写只需推进
//! mutation generation 并通知协调器；写操作不等待 Scan，Scan Superseded 后
//! 协作取消。" The coordinator freezes this generation at Run start; product
//! write actions bump it after a successful commit and the Run's watchdog
//! observes the change within one tick.

use std::sync::atomic::{AtomicU64, Ordering};

pub struct ScanMutationCoordinator {
    generation: AtomicU64,
}

impl ScanMutationCoordinator {
    pub fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Record that a product write that may change Scan-visible facts
    /// (skill trees, activations, Roots, installer locks) just committed.
    /// Product writes never wait for the Scan; the next watchdog tick
    /// Supersedes the Run and cleans its temporary evidence.
    pub fn bump(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Release) + 1
    }
}

impl Default for ScanMutationCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_bumps_monotonically() {
        let coordinator = ScanMutationCoordinator::new();
        assert_eq!(coordinator.generation(), 0);
        assert_eq!(coordinator.bump(), 1);
        assert_eq!(coordinator.bump(), 2);
        assert_eq!(coordinator.generation(), 2);
    }
}
