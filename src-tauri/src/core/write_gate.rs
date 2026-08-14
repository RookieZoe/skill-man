//! The single write authority for all product write modules (spec §4.3).
//!
//! `WriteGate` is an unforgeable Core capability, not a UI disabled flag:
//! every write Module is constructed with the shared gate, every Apply
//! re-reads the gate generation before commit, and any state transition
//! invalidates earlier plan tokens with `PlanStale`. `Recovery` is the only
//! narrow capability that lets Fixture Recovery / Restore write while regular
//! product writes stay closed.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::home::BoundHome;
use thiserror::Error;

/// Why regular product writes are refused while reads may continue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadOnlyReason {
    /// Catalog schema is newer than this build supports.
    UnsupportedSchema,
    /// SQLite integrity/foreign-key verification failed.
    IntegrityFailed,
    /// The writable open failed after identity verification (permission,
    /// lock, transient I/O); the session continues read-only.
    OpenFailed,
}

/// Why all regular product writes are refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClosedReason {
    Unconfigured,
    /// The previous Home was explicitly abandoned; only a brand-new binding
    /// (never the abandoned site) may proceed.
    Abandoned,
    AppStateUnavailable,
    LegacyDetected,
    FixtureRecoveryLocked,
    HomeUnavailable,
    HomeIdentityMismatch,
    HomeCandidatePending,
}

/// The gate state; each variant carries only the typed fields rendering and
/// the next legal action need.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteGateState {
    /// Regular product writes are allowed for this verified Home.
    Open(BoundHome),
    /// Only Catalog reads are allowed (spec "Catalog ReadOnly").
    CatalogReadOnly { reason: ReadOnlyReason },
    /// All regular product writes are refused.
    Closed { reason: ClosedReason },
    /// Only the named Fixture Recovery / Restore operation may write.
    Recovery { operation_id: String },
}

/// A point-in-time gate view: state plus the generation that plan tokens bind
/// to. Any transition bumps the generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GateSnapshot {
    pub state: WriteGateState,
    pub generation: u64,
}

#[derive(Debug, Error)]
pub enum WriteGateError {
    #[error("the write gate lock is poisoned")]
    Poisoned,
}

/// A plan token records the gate generation it was issued under; Apply must
/// re-validate it immediately before commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanTicket {
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanCheck {
    Current,
    Stale,
}

pub struct WriteGate {
    state: RwLock<WriteGateState>,
    generation: AtomicU64,
    /// The most recently opened Bound Home; lets `mark_ready` reopen the
    /// product-write path after startup recovery without re-verifying.
    last_bound: RwLock<Option<BoundHome>>,
}

impl WriteGate {
    pub fn new(state: WriteGateState) -> Self {
        let gate = Self {
            state: RwLock::new(state),
            generation: AtomicU64::new(0),
            last_bound: RwLock::new(None),
        };
        // Remember an Open initial state so `mark_ready` can reopen it.
        if let WriteGateState::Open(bound) = &gate.snapshot().state {
            if let Ok(mut last_bound) = gate.last_bound.write() {
                *last_bound = Some(bound.clone());
            }
        }
        gate
    }

    /// Test composition only: an open gate over a dummy Home so services can
    /// be exercised without a bootstrap authority. Production composition
    /// always derives the initial state from `BootstrapSnapshot`.
    pub fn open_for_tests() -> Self {
        Self::new(WriteGateState::Open(BoundHome::test_value(
            "00000000-0000-4000-8000-000000000000",
            std::path::PathBuf::from("/tmp/skill-man-test-home"),
        )))
    }

    pub fn snapshot(&self) -> GateSnapshot {
        let state = self
            .state
            .read()
            .map(|state| state.clone())
            .unwrap_or_else(|_| WriteGateState::Closed {
                reason: ClosedReason::AppStateUnavailable,
            });
        GateSnapshot {
            state,
            generation: self.generation(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Apply a new state; bumps the generation so outstanding plan tokens go
    /// stale. The transition itself is the event that re-renders bootstrap
    /// routes.
    pub fn transition_to(&self, next: WriteGateState) -> Result<GateSnapshot, WriteGateError> {
        let mut state = self.state.write().map_err(|_| WriteGateError::Poisoned)?;
        if let WriteGateState::Open(bound) = &next {
            if let Ok(mut last_bound) = self.last_bound.write() {
                *last_bound = Some(bound.clone());
            }
        }
        *state = next;
        drop(state);
        self.generation.fetch_add(1, Ordering::Release);
        Ok(self.snapshot())
    }

    /// Lock product writes for startup recovery: the operation that failed
    /// cannot compensate, so only the narrow recovery capability may write
    /// until `mark_ready` (spec §4.3 `Recovery`).
    pub fn mark_blocked(&self) {
        let _ = self.transition_to(WriteGateState::Recovery {
            operation_id: "startup-recovery".into(),
        });
    }

    /// Reopen product writes after successful startup recovery; no-op when no
    /// bound Home has been opened this session.
    pub fn mark_ready(&self) {
        let bound = self
            .last_bound
            .read()
            .map(|last| last.clone())
            .unwrap_or(None);
        if let Some(bound) = bound {
            let _ = self.transition_to(WriteGateState::Open(bound));
        }
    }

    /// Regular product writes are allowed only in `Open`; `Recovery` grants
    /// the narrow recovery capability, never general product writes.
    pub fn is_product_write_open(&self) -> bool {
        matches!(self.snapshot().state, WriteGateState::Open(_))
    }

    /// Re-read the gate generation: a plan token issued under an older
    /// generation is stale and its Apply must be refused.
    pub fn check_plan(&self, ticket: PlanTicket) -> PlanCheck {
        if ticket.generation == self.generation() {
            PlanCheck::Current
        } else {
            PlanCheck::Stale
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound() -> BoundHome {
        BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            std::path::PathBuf::from("/tmp/skill-man-home"),
        )
    }

    #[test]
    fn fresh_gate_has_generation_zero_and_opens_product_writes_only_in_open() {
        let gate = WriteGate::new(WriteGateState::Open(bound()));
        assert_eq!(gate.generation(), 0);
        assert!(gate.is_product_write_open());

        let closed = WriteGate::new(WriteGateState::Closed {
            reason: ClosedReason::Unconfigured,
        });
        assert!(!closed.is_product_write_open());

        let read_only = WriteGate::new(WriteGateState::CatalogReadOnly {
            reason: ReadOnlyReason::UnsupportedSchema,
        });
        assert!(!read_only.is_product_write_open());

        let recovery = WriteGate::new(WriteGateState::Recovery {
            operation_id: "op-1".into(),
        });
        assert!(
            !recovery.is_product_write_open(),
            "recovery is narrow, not product-open"
        );
    }

    #[test]
    fn transition_bumps_generation_and_stales_plan_tickets() {
        let gate = WriteGate::new(WriteGateState::Closed {
            reason: ClosedReason::Unconfigured,
        });
        let ticket = PlanTicket {
            generation: gate.generation(),
        };
        assert_eq!(gate.check_plan(ticket), PlanCheck::Current);

        gate.transition_to(WriteGateState::Open(bound()))
            .expect("transition to Open");
        assert_eq!(gate.generation(), 1);
        assert_eq!(gate.check_plan(ticket), PlanCheck::Stale);

        // A ticket issued after the transition is current again.
        let fresh = PlanTicket {
            generation: gate.generation(),
        };
        assert_eq!(gate.check_plan(fresh), PlanCheck::Current);
        gate.transition_to(WriteGateState::CatalogReadOnly {
            reason: ReadOnlyReason::OpenFailed,
        })
        .expect("transition to ReadOnly");
        assert_eq!(gate.check_plan(fresh), PlanCheck::Stale);
    }

    #[test]
    fn snapshot_carries_state_and_generation() {
        let gate = WriteGate::new(WriteGateState::Closed {
            reason: ClosedReason::HomeIdentityMismatch,
        });
        let snapshot = gate.snapshot();
        assert_eq!(
            snapshot.state,
            WriteGateState::Closed {
                reason: ClosedReason::HomeIdentityMismatch
            }
        );
        assert_eq!(snapshot.generation, 0);
    }

    #[test]
    fn mark_blocked_locks_product_writes_and_mark_ready_reopens_last_bound() {
        let gate = WriteGate::new(WriteGateState::Open(bound()));
        assert!(gate.is_product_write_open());

        gate.mark_blocked();
        assert!(!gate.is_product_write_open());
        assert!(matches!(
            gate.snapshot().state,
            WriteGateState::Recovery { .. }
        ));

        gate.mark_ready();
        assert!(gate.is_product_write_open());
        assert_eq!(gate.snapshot().state, WriteGateState::Open(bound()));
    }
}
