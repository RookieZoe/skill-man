//! The single write authority for all product write modules (spec §4.3).
//!
//! `WriteGate` is an unforgeable Core capability, not a UI disabled flag:
//! every write Module is constructed with the shared gate, every Apply
//! re-reads the gate generation before commit, and any state transition
//! invalidates earlier plan tokens with `PlanStale`. `Recovery` is the only
//! narrow capability that lets Fixture Recovery / Restore write while regular
//! product writes stay closed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock};

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
    DefaultHomeRecoveryOffer,
    DefaultHomeRecoveryBlocked,
    /// The previous Home was explicitly abandoned; only a brand-new binding
    /// (never the abandoned site) may proceed.
    Abandoned,
    AppStateUnavailable,
    LegacyDetected,
    FixtureRecoveryLocked,
    HomeUnavailable,
    HomeIdentityMismatch,
    HomeCandidatePending,
    /// A Home identity transition is between its invalidation and durable
    /// locator/store reconciliation. Product writes stay closed throughout.
    HomeTransition,
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
    #[error("no Bound Home is active")]
    NoBoundHome,
    #[error("the write gate is closed for product writes")]
    Closed,
    #[error("the Bound Home or WriteGate generation changed")]
    Stale,
}

/// A plan token records the gate generation it was issued under; Apply must
/// re-validate it immediately before commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanTicket {
    pub generation: u64,
}

/// The immutable Home identity and WriteGate generation a long-running
/// product operation was admitted under. A product write must acquire a
/// `ProductWriteGuard` with this context immediately before its first durable
/// mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomeWriteContext {
    pub home: BoundHome,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanCheck {
    Current,
    Stale,
}

/// A product mutation holds the shared side of the Home transition barrier.
/// Home transitions take the exclusive side before invalidating the context,
/// so a transition cannot commit while a product operation is in its durable
/// mutation phase.
pub struct ProductWriteGuard<'a> {
    _barrier: MutexGuard<'a, ()>,
}

/// Exclusive Home transition barrier. The constructor closes product writes
/// and bumps the generation; dropping without `commit` restores the previous
/// state conservatively, while `commit` leaves the gate closed for the caller
/// to reconcile with Bootstrap after the durable Home transition.
pub struct HomeTransitionGuard<'a> {
    gate: &'a WriteGate,
    _barrier: MutexGuard<'a, ()>,
    previous_state: WriteGateState,
    previous_bound: Option<BoundHome>,
    committed: bool,
}

pub struct WriteGate {
    state: RwLock<WriteGateState>,
    generation: AtomicU64,
    mutation_barrier: Mutex<()>,
    /// The most recently opened Bound Home; lets `mark_ready` reopen the
    /// product-write path after startup recovery without re-verifying.
    last_bound: RwLock<Option<BoundHome>>,
}

impl WriteGate {
    pub fn new(state: WriteGateState) -> Self {
        let gate = Self {
            state: RwLock::new(state),
            generation: AtomicU64::new(0),
            mutation_barrier: Mutex::new(()),
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

    /// Freeze the currently verified Home and gate generation for a
    /// long-running product operation. The caller may perform read-only or
    /// remote I/O before acquiring the durable mutation guard.
    pub fn capture_open_context(&self) -> Result<HomeWriteContext, WriteGateError> {
        let state = self.state.read().map_err(|_| WriteGateError::Poisoned)?;
        match &*state {
            WriteGateState::Open(home) => Ok(HomeWriteContext {
                home: home.clone(),
                generation: self.generation(),
            }),
            _ => Err(WriteGateError::Closed),
        }
    }

    /// Admit the operation's durable mutation phase. The shared barrier is
    /// held until the returned guard is dropped, making the context check and
    /// the first mutation one indivisible boundary with Home transitions.
    pub fn acquire_product_write(
        &self,
        context: &HomeWriteContext,
    ) -> Result<ProductWriteGuard<'_>, WriteGateError> {
        let barrier = self
            .mutation_barrier
            .lock()
            .map_err(|_| WriteGateError::Poisoned)?;
        self.validate_open_context(context)?;
        Ok(ProductWriteGuard { _barrier: barrier })
    }

    /// Check a frozen context without holding the mutation barrier. This is
    /// useful for read-only revalidation before the operation acquires its
    /// durable mutation phase; callers must still use
    /// `acquire_product_write` before the first mutation.
    pub fn validate_open_context(&self, context: &HomeWriteContext) -> Result<(), WriteGateError> {
        let state = self.state.read().map_err(|_| WriteGateError::Poisoned)?;
        let generation = self.generation();
        match &*state {
            WriteGateState::Open(home)
                if generation == context.generation && home == &context.home =>
            {
                Ok(())
            }
            WriteGateState::Open(_) => Err(WriteGateError::Stale),
            _ if generation != context.generation => Err(WriteGateError::Stale),
            _ => Err(WriteGateError::Closed),
        }
    }

    /// Start a Home identity transition. The exclusive barrier waits for an
    /// admitted product mutation to finish, then closes new product writes
    /// and invalidates all frozen contexts before the caller touches the
    /// locator or swaps the runtime Catalog.
    pub fn begin_home_transition(&self) -> Result<HomeTransitionGuard<'_>, WriteGateError> {
        self.begin_home_transition_inner(true)
    }

    /// Acquire the same exclusive Home barrier for a mutation that does not
    /// own the identity transition itself (for example, qualified Snapshot
    /// deletion). It still refuses startup/Fixture recovery ownership.
    pub fn begin_exclusive_home_write(&self) -> Result<HomeTransitionGuard<'_>, WriteGateError> {
        self.begin_home_transition_inner(false)
    }

    fn begin_home_transition_inner(
        &self,
        reject_transition_owner: bool,
    ) -> Result<HomeTransitionGuard<'_>, WriteGateError> {
        let barrier = self
            .mutation_barrier
            .lock()
            .map_err(|_| WriteGateError::Poisoned)?;
        let previous_state = self
            .state
            .read()
            .map_err(|_| WriteGateError::Poisoned)?
            .clone();
        if matches!(previous_state, WriteGateState::Recovery { .. })
            || (reject_transition_owner
                && matches!(
                    previous_state,
                    WriteGateState::Closed {
                        reason: ClosedReason::HomeTransition | ClosedReason::HomeCandidatePending
                    }
                ))
        {
            return Err(WriteGateError::Closed);
        }
        let previous_bound = self
            .last_bound
            .read()
            .map_err(|_| WriteGateError::Poisoned)?
            .clone();
        self.set_state(WriteGateState::Closed {
            reason: ClosedReason::HomeTransition,
        })?;
        self.generation.fetch_add(1, Ordering::Release);
        Ok(HomeTransitionGuard {
            gate: self,
            _barrier: barrier,
            previous_state,
            previous_bound,
            committed: false,
        })
    }

    /// Reconcile state and verified Home identity in one gate operation.
    /// Bootstrap callers must use this instead of publishing a new
    /// `last_bound` before closing or opening the corresponding state.
    pub fn reconcile(
        &self,
        desired: WriteGateState,
        bound_home: Option<&BoundHome>,
    ) -> Result<GateSnapshot, WriteGateError> {
        let _barrier = self
            .mutation_barrier
            .lock()
            .map_err(|_| WriteGateError::Poisoned)?;
        let mut state = self.state.write().map_err(|_| WriteGateError::Poisoned)?;
        let mut last_bound = self
            .last_bound
            .write()
            .map_err(|_| WriteGateError::Poisoned)?;
        let recovery_owned = matches!(*state, WriteGateState::Recovery { .. });
        let bound = bound_home.cloned();
        let (state_changed, bound_changed) = if recovery_owned {
            (false, false)
        } else {
            let state_changed = *state != desired;
            let bound_changed = *last_bound != bound;
            *state = desired;
            *last_bound = bound;
            (state_changed, bound_changed)
        };
        drop(last_bound);
        drop(state);
        if state_changed || bound_changed {
            self.generation.fetch_add(1, Ordering::Release);
        }
        Ok(self.snapshot())
    }

    /// Resolve the bootstrap authority and reconcile it while holding the
    /// Home mutation barrier. This prevents a stale snapshot read before the
    /// barrier from reopening a Home that was abandoned while the caller was
    /// waiting.
    pub fn reconcile_with<T, F>(&self, resolve: F) -> Result<(GateSnapshot, T), WriteGateError>
    where
        F: FnOnce(&WriteGateState) -> (WriteGateState, Option<BoundHome>, T),
    {
        let _barrier = self
            .mutation_barrier
            .lock()
            .map_err(|_| WriteGateError::Poisoned)?;
        let current_state = self
            .state
            .read()
            .map_err(|_| WriteGateError::Poisoned)?
            .clone();
        let (desired, bound, value) = resolve(&current_state);
        let mut state = self.state.write().map_err(|_| WriteGateError::Poisoned)?;
        let recovery_owned = matches!(*state, WriteGateState::Recovery { .. });
        let mut changed = false;
        if !recovery_owned {
            let mut last_bound = self
                .last_bound
                .write()
                .map_err(|_| WriteGateError::Poisoned)?;
            let state_changed = *state != desired;
            let bound_changed = *last_bound != bound;
            *state = desired;
            *last_bound = bound;
            changed = state_changed || bound_changed;
            drop(last_bound);
        }
        drop(state);
        if changed {
            self.generation.fetch_add(1, Ordering::Release);
        }
        Ok((self.snapshot(), value))
    }

    /// Apply a new state; bumps the generation so outstanding plan tokens go
    /// stale. The transition itself is the event that re-renders bootstrap
    /// routes.
    pub fn transition_to(&self, next: WriteGateState) -> Result<GateSnapshot, WriteGateError> {
        let _barrier = self
            .mutation_barrier
            .lock()
            .map_err(|_| WriteGateError::Poisoned)?;
        self.set_state(next)?;
        self.generation.fetch_add(1, Ordering::Release);
        Ok(self.snapshot())
    }

    fn set_state(&self, next: WriteGateState) -> Result<(), WriteGateError> {
        let mut state = self.state.write().map_err(|_| WriteGateError::Poisoned)?;
        if let WriteGateState::Open(bound) = &next {
            let mut last_bound = self
                .last_bound
                .write()
                .map_err(|_| WriteGateError::Poisoned)?;
            *last_bound = Some(bound.clone());
        }
        *state = next;
        Ok(())
    }

    /// Lock product writes for startup recovery: the operation that failed
    /// cannot compensate, so only the narrow recovery capability may write
    /// until `mark_ready` (spec §4.3 `Recovery`).
    pub fn mark_blocked(&self) {
        let Ok(mut state) = self.state.write() else {
            return;
        };
        if matches!(
            *state,
            WriteGateState::Recovery { .. }
                | WriteGateState::Closed {
                    reason: ClosedReason::HomeTransition
                }
        ) {
            return;
        }
        *state = WriteGateState::Recovery {
            operation_id: "startup-recovery".into(),
        };
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Reopen product writes after successful startup recovery; no-op when no
    /// bound Home has been opened this session.
    pub fn mark_ready(&self) {
        let Ok(_barrier) = self.mutation_barrier.lock() else {
            return;
        };
        let bound = self
            .last_bound
            .read()
            .map(|last| last.clone())
            .unwrap_or(None);
        let Some(bound) = bound else {
            return;
        };
        let Ok(mut state) = self.state.write() else {
            return;
        };
        if matches!(
            *state,
            WriteGateState::Recovery {
                ref operation_id
            } if operation_id == "startup-recovery"
        ) {
            *state = WriteGateState::Open(bound);
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    /// Align the runtime Home context with the bootstrap authority. A closed
    /// gate must not retain a previously abandoned or unavailable Home path:
    /// Home-scoped modules use this only after the bootstrap route has
    /// verified the current identity.
    pub fn synchronize_bound_home(
        &self,
        bound_home: Option<&BoundHome>,
    ) -> Result<(), WriteGateError> {
        let _barrier = self
            .mutation_barrier
            .lock()
            .map_err(|_| WriteGateError::Poisoned)?;
        let mut last_bound = self
            .last_bound
            .write()
            .map_err(|_| WriteGateError::Poisoned)?;
        let bound = bound_home.cloned();
        let changed = *last_bound != bound;
        *last_bound = bound;
        drop(last_bound);
        if changed {
            self.generation.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    /// The currently verified Home identity and path. This is deliberately
    /// separate from `snapshot`: a Catalog-read-only session still has a
    /// verified Home, while unavailable and abandoned states do not.
    pub fn bound_home(&self) -> Result<BoundHome, WriteGateError> {
        self.last_bound
            .read()
            .map_err(|_| WriteGateError::Poisoned)?
            .clone()
            .ok_or(WriteGateError::NoBoundHome)
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

impl HomeTransitionGuard<'_> {
    /// Leave product writes closed. The caller must reconcile the runtime
    /// Catalog and Bootstrap state before reopening the gate.
    pub fn commit(mut self) {
        self.committed = true;
    }

    /// Change the guarded state while retaining the exclusive barrier. The
    /// caller must later `commit` or `commit_to` this guard after the
    /// operation's durable phase finishes.
    pub fn set_state_while_held(&mut self, next: WriteGateState) -> Result<(), WriteGateError> {
        self.gate.set_state(next)?;
        self.gate.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Transfer the exclusive Home-transition claim to a durable recovery
    /// owner before releasing the barrier.
    pub fn commit_to(mut self, next: WriteGateState) -> Result<(), WriteGateError> {
        self.gate.set_state(next)?;
        self.gate.generation.fetch_add(1, Ordering::Release);
        self.committed = true;
        Ok(())
    }
}

impl Drop for HomeTransitionGuard<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // A failed Home transition restores the exact prior capability but
        // still bumps the generation, so every context observed while the
        // transition was in flight remains stale.
        if let Ok(mut state) = self.gate.state.write() {
            *state = self.previous_state.clone();
        }
        if let Ok(mut bound) = self.gate.last_bound.write() {
            *bound = self.previous_bound.clone();
        }
        self.gate.generation.fetch_add(1, Ordering::Release);
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

    #[test]
    fn synchronized_bound_home_never_survives_a_closed_bootstrap_state() {
        let gate = WriteGate::new(WriteGateState::Closed {
            reason: ClosedReason::Unconfigured,
        });
        assert!(matches!(
            gate.bound_home(),
            Err(WriteGateError::NoBoundHome)
        ));

        gate.synchronize_bound_home(Some(&bound()))
            .expect("record verified Home");
        assert_eq!(gate.bound_home().expect("verified Home"), bound());

        gate.synchronize_bound_home(None)
            .expect("clear unbound Home");
        assert!(matches!(
            gate.bound_home(),
            Err(WriteGateError::NoBoundHome)
        ));
    }

    #[test]
    fn a_home_transition_invalidates_the_frozen_write_context() {
        let gate = WriteGate::new(WriteGateState::Open(bound()));
        let context = gate.capture_open_context().expect("open context");

        let transition = gate.begin_home_transition().expect("home transition");
        assert!(
            !gate.is_product_write_open(),
            "a Home transition closes ordinary product writes before its CAS"
        );
        transition.commit();

        assert!(matches!(
            gate.acquire_product_write(&context),
            Err(WriteGateError::Stale)
        ));
    }

    #[test]
    fn home_transition_waits_for_an_in_flight_product_write() {
        let gate = std::sync::Arc::new(WriteGate::new(WriteGateState::Open(bound())));
        let context = gate.capture_open_context().expect("open context");
        let permit = gate
            .acquire_product_write(&context)
            .expect("product write permit");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let transition_gate = gate.clone();
        let transition = std::thread::spawn(move || {
            started_tx.send(()).expect("transition started");
            let transition = transition_gate
                .begin_home_transition()
                .expect("home transition");
            release_rx.recv().expect("release transition");
            transition.commit();
        });

        started_rx.recv().expect("transition thread started");
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(
            gate.is_product_write_open(),
            "the transition cannot invalidate an in-flight write"
        );
        drop(permit);
        release_tx.send(()).expect("release transition");
        transition.join().expect("transition thread");
        assert!(!gate.is_product_write_open());
    }

    #[test]
    fn bootstrap_reconcile_cannot_reopen_a_recovery_owned_gate() {
        let gate = WriteGate::new(WriteGateState::Recovery {
            operation_id: "recovery-1".into(),
        });
        let bound = bound();
        gate.synchronize_bound_home(Some(&bound))
            .expect("verified recovery Home");

        let (snapshot, ()) = gate
            .reconcile_with(|_| (WriteGateState::Open(bound.clone()), Some(bound.clone()), ()))
            .expect("reconcile");
        assert!(matches!(snapshot.state, WriteGateState::Recovery { .. }));
        assert!(!gate.is_product_write_open());
    }

    #[test]
    fn bootstrap_reconcile_rechecks_a_gate_blocked_during_snapshot_resolution() {
        let gate = std::sync::Arc::new(WriteGate::new(WriteGateState::Open(bound())));
        let bound = bound();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let resolving_gate = gate.clone();
        let resolver = std::thread::spawn(move || {
            resolving_gate
                .reconcile_with(|_| {
                    entered_tx.send(()).expect("resolver entered");
                    release_rx.recv().expect("release resolver");
                    (WriteGateState::Open(bound.clone()), Some(bound), ())
                })
                .expect("reconcile")
        });

        entered_rx.recv().expect("resolver entered");
        gate.mark_blocked();
        release_tx.send(()).expect("release resolver");
        let snapshot = resolver.join().expect("resolver thread");
        assert!(matches!(snapshot.0.state, WriteGateState::Recovery { .. }));
        assert!(!gate.is_product_write_open());
    }
}
