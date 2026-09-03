//! Rescan Run lifecycle and Scan Evidence Store coordination (spec §4.10,
//! ADR-0020): the single Core Module for manual full Rescan, single-flight
//! Run state machine, streaming evidence, progress, cancel and stale
//! coordination. React, onboarding, Agent Management and Adopt all share
//! this Interface and never compose their own filesystem loops.
//!
//! Contracts enforced here:
//! - one active Run per Bound Home; repeated triggers reuse the current Run;
//! - a Run freezes `home_id`, WriteGate generation, Agent Configuration
//!   generation, the configured canonical Root snapshot and the Scan
//!   mutation generation; any change Supersedes + cancels it;
//! - no Root/entry/entity/file/byte business cap and no total hard deadline;
//!   only the bounded concurrency (Root 4 / tree hash 2 / local Git probe 2),
//!   per-Root progress watchdogs, symlink 16-hop/cycle and Root containment
//!   safety boundaries;
//! - progress events carry only real phase/Root/count/elapsed, published at
//!   most every 250 ms (phase/status changes immediately), never percent or
//!   ETA;
//! - a Root is the minimum evidence commit unit: a failed Root publishes no
//!   half candidate; healthy Roots still form an Incomplete Report; a
//!   store-wide failure fails the whole Run and keeps the old Report.

pub mod classification;
pub mod engine;
pub mod mutation;
pub mod plan;
pub mod qualifier;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use crate::core::home::BoundHome;
use crate::core::write_gate::{WriteGate, WriteGateState};
use crate::seams::agent_configuration_store::AgentConfigurationStore;
use crate::seams::app_state_store::AppStateStore;
use crate::seams::clock::Clock;
use crate::seams::filesystem::FileSystem;
use crate::seams::installer_lock_store::InstallerLockStore;
use crate::seams::local_git_probe::LocalGitProbe;
use crate::seams::scan_evidence_store::{
    CurrentManifestRead, ScanEvidenceCounts, ScanEvidenceStore, ScanEvidenceStoreFactory,
    ScanFrozenFacts, ScanFrozenRoot, ScanReportManifest, ScanReportPageRead, ScanRootAgentRef,
    ScanRunRecord, ScanSourceCounts,
};
use crate::seams::scan_integrity::canonical_json_digest;
use crate::seams::scan_managed_facts::ScanManagedFactsReader;
use thiserror::Error;

use self::mutation::ScanMutationCoordinator;
use self::plan::PlannedRoot;

/// Closed Run lifecycle states (spec §4.10): queued → running →
/// cancelling → cancelled | superseded | completed | failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanRunState {
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Superseded,
    Completed,
    Failed,
}

impl ScanRunState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Superseded | Self::Completed | Self::Failed
        )
    }
}

/// The current Run phase; progress is real evidence, never a promise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanPhase {
    Planning,
    Walking,
    Hashing,
    Finalizing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanTrigger {
    Onboarding,
    Manual,
}

impl ScanTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Onboarding => "onboarding",
            Self::Manual => "manual",
        }
    }
}

/// Root view state while a Run is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanRootViewState {
    Pending,
    Walking,
    Completed,
    Failed,
    Unresponsive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanRootView {
    pub index: u32,
    pub configured_path: PathBuf,
    pub canonical_path: PathBuf,
    pub state: ScanRootViewState,
    pub counts: ScanEvidenceCounts,
    pub elapsed_ms: u64,
    pub slow: bool,
    pub diagnostic: Option<String>,
}

/// Bounded Run summary; the evidence itself is streamed to disk (spec §3.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanRunSnapshot {
    pub run_id: String,
    pub generation: u64,
    pub trigger: ScanTrigger,
    pub state: ScanRunState,
    pub phase: ScanPhase,
    pub current_root: Option<u32>,
    pub counts: ScanEvidenceCounts,
    pub roots: Vec<ScanRootView>,
    pub elapsed_ms: u64,
    pub slow: bool,
    pub diagnostic: Option<String>,
}

/// Layered Root coverage of a terminal Report (spec §4.6 `coverage_counts`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanCoverageCounts {
    pub completed: u64,
    pub failed: u64,
    pub unresponsive: u64,
}

/// Bounded summary of the terminal Report (spec §4.6): the full Root
/// coverage/entity/appearance/diagnostic detail is served by the unique
/// `report_page` contract, never by the summary DTO.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanReportSummary {
    pub generation: u64,
    pub run_id: String,
    /// The unforgeable Report identity page cursors bind to.
    pub content_identity: String,
    pub trigger: ScanTrigger,
    pub state: ScanReportState,
    pub coverage: ScanCoverageCounts,
    pub counts: ScanEvidenceCounts,
    pub incomplete: bool,
    pub published_at_ms: u64,
    pub agent_configuration_generation: u64,
    pub configured_root_snapshot_fingerprint: String,
    pub started_at_ms: u64,
    pub slow: bool,
    /// Bounded source classification counts (spec §8.1); the candidate
    /// detail is served by `report_page`, never by this summary.
    pub source_counts: ScanSourceCounts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanReportState {
    Complete,
    Incomplete,
}

/// Why a terminal Report is not current evidence (ADR-0020 §UI).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaleReason {
    /// Loaded from disk in a different process session.
    CrossStartup,
    /// Agent Configuration generation changed after the Report.
    ConfigurationChanged,
    /// Home identity or WriteGate generation changed after the Report.
    HomeOrGateChanged,
    /// A product write advanced the Scan mutation generation.
    FilesystemChanged,
    /// The current manifest on disk is corrupt/torn; no Report is shown.
    CacheUnreadable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportFreshness {
    Current,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentReportView {
    pub summary: Option<ScanReportSummary>,
    pub freshness: ReportFreshness,
    pub stale_reasons: Vec<StaleReason>,
}

/// The scan slice of the Observation snapshot (bounded).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanCoordinatorSnapshot {
    pub run: Option<ScanRunSnapshot>,
    pub current_report: CurrentReportView,
}

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("full Rescan requires WriteGate::Open: {0}")]
    NotWritable(String),
    #[error("the Agent Configuration snapshot is unavailable: {0}")]
    ConfigurationUnavailable(String),
    #[error("the Scan Evidence Store is unavailable: {0}")]
    StoreUnavailable(String),
    #[error("no active Run with id {0}")]
    RunNotFound(String),
    #[error("internal Scan Run error: {0}")]
    Internal(String),
    #[error("the Scan Run was superseded by a product write")]
    Superseded,
    #[error("the Report page cursor is stale: current generation {0}")]
    ReportPageStale(u64),
    #[error("the Report page is not available")]
    ReportPageNotFound,
}

/// Progress observer fed by the Run threads; the Tauri adapter converts the
/// snapshot to the shared `observation://changed` DTO (≤250 ms throttle
/// happens in the coordinator).
pub trait ScanRunObserver: Send + Sync {
    fn on_scan_change(&self, snapshot: &ScanRunSnapshot);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunFrozen {
    pub facts: ScanFrozenFacts,
    pub roots: Vec<ScanFrozenRoot>,
    pub home: BoundHome,
}

pub(crate) struct RunShared {
    pub(crate) progress: Mutex<RunProgress>,
    pub(crate) cancel: AtomicBool,
    /// The Run thread finished its walk; the watchdog must exit even though
    /// the progress state is only set terminal in finalize (after the join).
    pub(crate) done: AtomicBool,
}

pub(crate) struct RootProgress {
    pub(crate) index: u32,
    pub(crate) configured_path: PathBuf,
    pub(crate) canonical_path: PathBuf,
    /// Frozen directory identity; re-verified before the Root walks.
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) state: ScanRootViewState,
    pub(crate) counts: ScanEvidenceCounts,
    pub(crate) started: Instant,
    pub(crate) slow: bool,
    pub(crate) diagnostic: Option<String>,
    pub(crate) last_progress: Instant,
    /// Consumer Agents of this Root (coverage rows; spec §7.6).
    pub(crate) consumer_agents: Vec<ScanRootAgentRef>,
}

pub(crate) struct RunProgress {
    pub(crate) state: ScanRunState,
    pub(crate) phase: ScanPhase,
    pub(crate) current_root: Option<u32>,
    pub(crate) counts: ScanEvidenceCounts,
    pub(crate) roots: Vec<RootProgress>,
    pub(crate) started: Instant,
    pub(crate) slow: bool,
    pub(crate) diagnostic: Option<String>,
    pub(crate) last_emit: Instant,
}

impl RunProgress {
    pub(crate) fn new(
        planned: Vec<PlannedRoot>,
        state: ScanRunState,
        configured_agents: u64,
        declared_roots: u64,
    ) -> Self {
        let canonical_roots = planned.iter().filter(|root| root.frozen.is_some()).count() as u64;
        let started = Instant::now();
        let root_progress: Vec<RootProgress> = planned
            .into_iter()
            .map(|root| {
                let walkable = root.frozen.clone();
                let configured_path = root.configured_path.clone();
                RootProgress {
                    index: root.index,
                    configured_path: configured_path.clone(),
                    canonical_path: walkable
                        .as_ref()
                        .map(|frozen| frozen.canonical_path.clone())
                        .unwrap_or(configured_path),
                    state: if walkable.is_some() {
                        ScanRootViewState::Pending
                    } else {
                        ScanRootViewState::Failed
                    },
                    device: walkable.as_ref().map(|frozen| frozen.device).unwrap_or(0),
                    inode: walkable.as_ref().map(|frozen| frozen.inode).unwrap_or(0),
                    counts: ScanEvidenceCounts::default(),
                    started,
                    slow: false,
                    diagnostic: root.plan_error,
                    last_progress: started,
                    consumer_agents: root.consumer_agents.clone(),
                }
            })
            .collect();
        let failed_roots = root_progress
            .iter()
            .filter(|root| root.state == ScanRootViewState::Failed)
            .count() as u64;
        Self {
            state,
            phase: ScanPhase::Planning,
            current_root: None,
            counts: ScanEvidenceCounts {
                roots: root_progress.len() as u64,
                failed_roots,
                configured_agents,
                declared_roots,
                canonical_roots,
                ..Default::default()
            },
            roots: root_progress,
            started,
            slow: false,
            diagnostic: None,
            last_emit: started,
        }
    }
}

pub(crate) struct RunSlot {
    pub(crate) record: Arc<ScanRunRecord>,
    pub(crate) store: Arc<dyn ScanEvidenceStore>,
    pub(crate) shared: Arc<RunShared>,
    pub(crate) handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl RunSlot {
    pub(crate) fn progress_lock(&self) -> std::sync::MutexGuard<'_, RunProgress> {
        self.shared
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) struct CoordinatorState {
    run: Option<Arc<RunSlot>>,
    /// Last terminal run retained for the ledger within this session.
    last_run: Option<ScanRunSnapshot>,
    /// Report published by a Run in this process (freshness Current).
    published_report: Option<Arc<ScanReportManifest>>,
    /// Report loaded once per process from disk (Stale).
    cross_startup_report: Option<Arc<ScanReportManifest>>,
    /// (home_id, marker_at_ms, newest_restore_at_ms) of the last startup
    /// marker work; keyed so the idempotent check repeats when facts move.
    marker_cache: Option<(String, u64, u64)>,
    next_generation: u64,
}

pub struct ScanCoordinator {
    pub(crate) factory: Arc<dyn ScanEvidenceStoreFactory>,
    pub(crate) filesystem: Arc<dyn FileSystem>,
    pub(crate) lock_store: Arc<dyn InstallerLockStore>,
    pub(crate) git_probe: Arc<dyn LocalGitProbe>,
    pub(crate) write_gate: Arc<WriteGate>,
    pub(crate) agent_store: Arc<dyn AgentConfigurationStore>,
    pub(crate) mutation: Arc<ScanMutationCoordinator>,
    pub(crate) app_state: Arc<dyn AppStateStore>,
    /// Read-only managed Skill facts (Catalog path authority) used by the
    /// classification pass of a Run (#84; spec §8.2).
    pub(crate) managed_facts: Arc<dyn ScanManagedFactsReader>,
    /// The App state directory: an entity inside it is inside a control
    /// zone (spec §8.2).
    pub(crate) app_state_path: PathBuf,
    pub(crate) clock: Arc<dyn Clock>,
    /// Zero-progress isolation window per Root (spec §4.10 default 30s);
    /// test composition shortens it to prove the typed Unresponsive path.
    pub(crate) unresponsive_ms: u64,
    state: Mutex<CoordinatorState>,
    observer: RwLock<Option<Arc<dyn ScanRunObserver>>>,
}

impl ScanCoordinator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        factory: Arc<dyn ScanEvidenceStoreFactory>,
        filesystem: Arc<dyn FileSystem>,
        lock_store: Arc<dyn InstallerLockStore>,
        git_probe: Arc<dyn LocalGitProbe>,
        write_gate: Arc<WriteGate>,
        agent_store: Arc<dyn AgentConfigurationStore>,
        mutation: Arc<ScanMutationCoordinator>,
        app_state: Arc<dyn AppStateStore>,
        managed_facts: Arc<dyn ScanManagedFactsReader>,
        app_state_path: PathBuf,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            factory,
            filesystem,
            lock_store,
            git_probe,
            write_gate,
            agent_store,
            mutation,
            app_state,
            managed_facts,
            app_state_path,
            clock,
            unresponsive_ms: 30_000,
            state: Mutex::new(CoordinatorState {
                run: None,
                last_run: None,
                published_report: None,
                cross_startup_report: None,
                marker_cache: None,
                next_generation: 0,
            }),
            observer: RwLock::new(None),
        }
    }

    /// Test composition: shorten the zero-progress isolation window.
    pub fn with_unresponsive_ms(mut self, milliseconds: u64) -> Self {
        self.unresponsive_ms = milliseconds;
        self
    }

    pub fn set_observer(&self, observer: Arc<dyn ScanRunObserver>) {
        *self
            .observer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(observer);
    }

    pub(crate) fn state_lock(&self) -> std::sync::MutexGuard<'_, CoordinatorState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Current bounded view: active/last Run plus the current Report view
    /// with computed freshness (spec §4.10).
    pub fn snapshot(&self) -> ScanCoordinatorSnapshot {
        // Startup marker maintenance is idempotent (keyed by home +
        // restore-generation); a same-session Restore re-opens the gate and
        // the next snapshot rewrites the marker after the restore.
        if let WriteGateState::Open(home) = &self.write_gate.snapshot().state {
            let _ = self.ensure_startup_marker(home);
        }
        let mut state = self.state_lock();
        let run = state
            .run
            .as_ref()
            .map(|slot| build_run_snapshot(slot))
            .or_else(|| state.last_run.clone());
        let (current_report, loaded) = self.compute_report_view(&state);
        if let Some(report) = loaded {
            state.cross_startup_report = Some(report);
        }
        ScanCoordinatorSnapshot {
            run,
            current_report,
        }
    }

    /// Start a full Rescan Run (single-flight): an active Run is reused
    /// (joined), a terminal Run starts a new generation.
    pub fn start_rescan(
        self: &Arc<Self>,
        trigger: ScanTrigger,
    ) -> Result<ScanCoordinatorSnapshot, ScanError> {
        let gate = self.write_gate.snapshot();
        let bound = match &gate.state {
            WriteGateState::Open(home) => home.clone(),
            other => {
                return Err(ScanError::NotWritable(
                    write_gate_state_summary(other).to_string(),
                ));
            }
        };
        // Startup marker maintenance is idempotent and cheap.
        let _ = self.ensure_startup_marker(&bound);
        let snapshot = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.run.is_some() {
                // Single-flight: repeated triggers just open current progress.
                let (current_report, loaded) = self.compute_report_view(&state);
                if let Some(report) = loaded {
                    state.cross_startup_report = Some(report);
                }
                return Ok(ScanCoordinatorSnapshot {
                    run: state.run.as_ref().map(|slot| build_run_snapshot(slot)),
                    current_report,
                });
            }
            let agent_snapshot = self
                .agent_store
                .agent_configuration_snapshot()
                .map_err(|error| ScanError::ConfigurationUnavailable(error.to_string()))?;
            let planned = plan::plan_roots_with_failures(self.filesystem.as_ref(), &agent_snapshot)
                .map_err(|error| ScanError::ConfigurationUnavailable(error.to_string()))?;
            let roots = planned
                .iter()
                .filter_map(|planned| planned.frozen.clone())
                .collect::<Vec<_>>();
            let configured_agents = agent_snapshot.configurations.len() as u64;
            let declared_roots = agent_snapshot.roots.len() as u64;
            let generation = state.next_generation.max(
                state
                    .cross_startup_report
                    .as_ref()
                    .map(|report| report.generation)
                    .unwrap_or(0),
            ) + 1;
            state.next_generation = generation;
            let run_id = self.new_run_id()?;
            let store = self
                .factory
                .store_for(&bound)
                .map_err(|error| ScanError::StoreUnavailable(error.to_string()))?;
            let fingerprint = plan::roots_fingerprint(&roots);
            let record = Arc::new(ScanRunRecord {
                schema_version: crate::seams::scan_evidence_store::SCAN_STORE_SCHEMA_VERSION,
                home_id: bound.home_id.0.clone(),
                run_id: run_id.clone(),
                generation,
                trigger: trigger.as_str().to_owned(),
                frozen: ScanFrozenFacts {
                    home_id: bound.home_id.0.clone(),
                    write_gate_generation: gate.generation,
                    agent_configuration_generation: agent_snapshot.snapshot_version,
                    mutation_generation: self.mutation.generation(),
                    roots_fingerprint: fingerprint,
                    configured_agents,
                    declared_roots,
                },
                roots: roots.clone(),
                started_at_ms: (self.clock.unix_epoch_nanos() / 1_000_000) as u64,
            });
            let slot = Arc::new(RunSlot {
                record: record.clone(),
                store,
                shared: Arc::new(RunShared {
                    progress: Mutex::new(RunProgress::new(
                        planned,
                        ScanRunState::Queued,
                        configured_agents,
                        declared_roots,
                    )),
                    cancel: AtomicBool::new(false),
                    done: AtomicBool::new(false),
                }),
                handle: Mutex::new(None),
            });
            state.run = Some(slot.clone());
            drop(state);
            engine::execute(self.clone(), slot.clone());
            let mut fresh = self.state_lock();
            let (current_report, loaded) = self.compute_report_view(&fresh);
            if let Some(report) = loaded {
                fresh.cross_startup_report = Some(report);
            }
            ScanCoordinatorSnapshot {
                run: Some(build_run_snapshot(&slot)),
                current_report,
            }
        };
        Ok(snapshot)
    }

    /// Coordinate cancellation of the active Run: workers stop at the next
    /// checkpoint, temporary evidence is removed and the previous Report is
    /// untouched (spec §4.10; ADR-0020).
    pub fn cancel_rescan(&self, run_id: &str) -> Result<ScanCoordinatorSnapshot, ScanError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(slot) = &state.run else {
            let (current_report, _) = self.compute_report_view(&state);
            return Ok(ScanCoordinatorSnapshot {
                run: state.last_run.clone(),
                current_report,
            });
        };
        if slot.record.run_id != run_id {
            return Err(ScanError::RunNotFound(run_id.to_owned()));
        }
        slot.shared
            .cancel
            .store(true, std::sync::atomic::Ordering::Release);
        let mut progress = slot
            .shared
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !progress.state.is_terminal() {
            progress.state = ScanRunState::Cancelling;
        }
        drop(progress);
        self.publish_progress(slot, true);
        let (current_report, _) = self.compute_report_view(&state);
        Ok(ScanCoordinatorSnapshot {
            run: Some(build_run_snapshot(slot)),
            current_report,
        })
    }

    /// The unique paged Report read contract (spec §4.10 `report_page`;
    /// ADR-0017): Root coverage, canonical entities, appearances and typed
    /// diagnostics of the current Report generation only. A cursor whose
    /// Report identity is no longer current returns typed stale; a corrupt
    /// manifest or missing artifact returns not-found — never a fallback to
    /// another Report.
    pub fn report_page(
        &self,
        cursor: crate::seams::scan_evidence_store::ScanReportCursor,
        limit: usize,
    ) -> Result<ScanReportPageRead, ScanError> {
        let gate = self.write_gate.snapshot();
        let bound = match &gate.state {
            WriteGateState::Open(home) => home.clone(),
            other => {
                return Err(ScanError::NotWritable(
                    write_gate_state_summary(other).to_string(),
                ));
            }
        };
        let store = self
            .factory
            .store_for(&bound)
            .map_err(|error| ScanError::StoreUnavailable(error.to_string()))?;
        store
            .report_page(&cursor, limit)
            .map_err(|error| match error {
                crate::seams::scan_evidence_store::ScanReportPageError::Stale {
                    current_generation,
                } => ScanError::ReportPageStale(current_generation),
                crate::seams::scan_evidence_store::ScanReportPageError::NotFound => {
                    ScanError::ReportPageNotFound
                }
            })
    }

    /// The 250 ms / phase-change progress publisher.
    pub(crate) fn publish_progress(&self, slot: &RunSlot, force: bool) {
        let publish = {
            let mut progress = slot
                .shared
                .progress
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let now = Instant::now();
            let due = now.duration_since(progress.last_emit).as_millis() >= 250;
            if due || force {
                progress.last_emit = now;
                Some(build_run_snapshot_locked(&progress, slot.record.as_ref()))
            } else {
                None
            }
        };
        if let Some(snapshot) = publish {
            if let Some(observer) = self
                .observer
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                observer.on_scan_change(&snapshot);
            }
        }
    }

    /// Freeze-check used by the watchdog: one of the frozen generations moved.
    pub(crate) fn frozen_superseded(&self, frozen: &ScanFrozenFacts) -> bool {
        let gate = self.write_gate.snapshot();
        let home = match &gate.state {
            WriteGateState::Open(home) => home.clone(),
            _ => return true,
        };
        if home.home_id.0 != frozen.home_id || gate.generation != frozen.write_gate_generation {
            return true;
        }
        if self.mutation.generation() != frozen.mutation_generation {
            return true;
        }
        self.agent_store
            .agent_configuration_snapshot()
            .map(|snapshot| snapshot.snapshot_version != frozen.agent_configuration_generation)
            .unwrap_or(true)
    }

    /// Idempotent durable startup marker: proves this Home reached Bound
    /// `WriteGate::Open` after its latest completed restore. The marker is
    /// rewritten when the Home has a newer restore operation than the last
    /// marker (same-session Restore → post-restore Open transitions).
    pub(crate) fn ensure_startup_marker(&self, bound: &BoundHome) -> Result<(), ScanError> {
        let ledger = self
            .app_state
            .load()
            .map_err(|error| ScanError::StoreUnavailable(error.to_string()))?
            .recovery_ledger;
        let newest_restore_ms = ledger
            .completed
            .iter()
            .filter(|record| {
                record.home_id.as_ref().map(|id| id.0.clone()) == Some(bound.home_id.0.clone())
                    && record.snapshot_path.is_some()
            })
            .filter_map(|record| rfc3339_to_epoch_millis(&record.created_at))
            .max()
            .unwrap_or(0);
        let marker_at_ms = self
            .factory
            .store_for(bound)
            .map_err(|error| ScanError::StoreUnavailable(error.to_string()))?
            .startup_marker()
            .map_err(|error| ScanError::StoreUnavailable(error.to_string()))?
            .map(|marker| marker.marked_at_ms)
            .unwrap_or(0);
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.marker_cache
                == Some((bound.home_id.0.clone(), marker_at_ms, newest_restore_ms))
            {
                return Ok(());
            }
        }
        if marker_at_ms >= newest_restore_ms && marker_at_ms > 0 {
            // Already fresh: the cache key can stay current.
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.marker_cache = Some((bound.home_id.0.clone(), marker_at_ms, newest_restore_ms));
            return Ok(());
        }
        let marker = crate::seams::scan_evidence_store::ScanStartupMarker {
            schema_version: crate::seams::scan_evidence_store::SCAN_STORE_SCHEMA_VERSION,
            home_id: bound.home_id.0.clone(),
            marked_at_ms: (self.clock.unix_epoch_nanos() / 1_000_000) as u64,
        };
        self.factory
            .store_for(bound)
            .map_err(|error| ScanError::StoreUnavailable(error.to_string()))?
            .write_startup_marker(&marker)
            .map_err(|error| ScanError::StoreUnavailable(error.to_string()))?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.marker_cache = Some((
            bound.home_id.0.clone(),
            marker.marked_at_ms,
            newest_restore_ms,
        ));
        Ok(())
    }

    fn new_run_id(&self) -> Result<String, ScanError> {
        let mut bytes = [0_u8; 16];
        self.filesystem
            .read_entropy(&mut bytes)
            .map_err(|error| ScanError::Internal(error.to_string()))?;
        Ok(crate::seams::clock::uuid_v4_shape(&mut bytes))
    }

    fn compute_report_view(
        &self,
        state: &CoordinatorState,
    ) -> (CurrentReportView, Option<Arc<ScanReportManifest>>) {
        // Reads from disk lazily once per process; corrupt cache is a
        // `No cached report` (fail-closed, never an Error). The loaded
        // manifest is returned so the caller caches it under its own lock.
        let load_cross_startup = || -> Option<Arc<ScanReportManifest>> {
            let gate = self.write_gate.snapshot();
            let WriteGateState::Open(home) = &gate.state else {
                return None;
            };
            match self.factory.store_for(home) {
                Ok(store) => match store.current_manifest() {
                    Ok(CurrentManifestRead::Report(manifest)) => Some(Arc::new(manifest)),
                    Ok(CurrentManifestRead::Absent) => None,
                    Ok(CurrentManifestRead::Corrupt) => None,
                    Err(_) => None,
                },
                Err(_) => None,
            }
        };
        let (manifest, loaded): (
            Option<Arc<ScanReportManifest>>,
            Option<Arc<ScanReportManifest>>,
        ) = if let Some(published) = &state.published_report {
            (Some(published.clone()), None)
        } else if let Some(cached) = &state.cross_startup_report {
            (Some(cached.clone()), None)
        } else {
            match load_cross_startup() {
                Some(report) => (Some(report.clone()), Some(report)),
                None => (None, None),
            }
        };
        let Some(manifest) = manifest else {
            return (
                CurrentReportView {
                    summary: None,
                    freshness: ReportFreshness::Stale,
                    stale_reasons: vec![StaleReason::CrossStartup],
                },
                loaded,
            );
        };
        let published_in_process = state
            .published_report
            .as_ref()
            .map(|report| report.content_identity == manifest.content_identity)
            .unwrap_or(false);
        let mut stale_reasons = Vec::new();
        if !published_in_process {
            stale_reasons.push(StaleReason::CrossStartup);
        }
        let gate = self.write_gate.snapshot();
        let bound_home = match &gate.state {
            WriteGateState::Open(home) => Some(home.clone()),
            _ => None,
        };
        let home_matches = bound_home
            .as_ref()
            .map(|home| home.home_id.0 == manifest.frozen.home_id)
            .unwrap_or(false);
        if !home_matches || gate.generation != manifest.frozen.write_gate_generation {
            stale_reasons.push(StaleReason::HomeOrGateChanged);
        }
        if self.mutation.generation() != manifest.frozen.mutation_generation {
            stale_reasons.push(StaleReason::FilesystemChanged);
        }
        if self
            .agent_store
            .agent_configuration_snapshot()
            .map(|snapshot| {
                snapshot.snapshot_version != manifest.frozen.agent_configuration_generation
            })
            .unwrap_or(true)
        {
            stale_reasons.push(StaleReason::ConfigurationChanged);
        }
        let freshness = if stale_reasons.is_empty() {
            ReportFreshness::Current
        } else {
            ReportFreshness::Stale
        };
        let mut coverage = ScanCoverageCounts::default();
        for root in &manifest.roots {
            match root.state {
                crate::seams::scan_evidence_store::ScanRootState::Completed => {
                    coverage.completed += 1;
                }
                crate::seams::scan_evidence_store::ScanRootState::Unresponsive => {
                    coverage.unresponsive += 1;
                }
                crate::seams::scan_evidence_store::ScanRootState::Failed => {
                    coverage.failed += 1;
                }
            }
        }
        let slow = manifest.ended_at_ms.saturating_sub(manifest.started_at_ms) > 30_000;
        (
            CurrentReportView {
                summary: Some(ScanReportSummary {
                    generation: manifest.generation,
                    run_id: manifest.run_id.clone(),
                    content_identity: manifest.content_identity.clone(),
                    trigger: parse_trigger(&manifest.trigger),
                    state: parse_report_state(&manifest.state),
                    coverage,
                    counts: manifest.counts,
                    incomplete: parse_report_state(&manifest.state) == ScanReportState::Incomplete,
                    published_at_ms: manifest.ended_at_ms,
                    agent_configuration_generation: manifest.frozen.agent_configuration_generation,
                    configured_root_snapshot_fingerprint: manifest.frozen.roots_fingerprint.clone(),
                    started_at_ms: manifest.started_at_ms,
                    slow,
                    source_counts: manifest.source_counts,
                }),
                freshness,
                stale_reasons,
            },
            loaded,
        )
    }
}

pub(crate) fn build_run_snapshot(slot: &RunSlot) -> ScanRunSnapshot {
    let progress = slot.progress_lock();
    build_run_snapshot_locked(&progress, slot.record.as_ref())
}

/// Snapshot builder over an already-held progress guard (no re-entrant lock).
fn build_run_snapshot_locked(progress: &RunProgress, record: &ScanRunRecord) -> ScanRunSnapshot {
    ScanRunSnapshot {
        run_id: record.run_id.clone(),
        generation: record.generation,
        trigger: parse_trigger(&record.trigger),
        state: progress.state,
        phase: progress.phase,
        current_root: progress.current_root,
        counts: progress.counts,
        roots: progress
            .roots
            .iter()
            .map(|root| ScanRootView {
                index: root.index,
                configured_path: root.configured_path.clone(),
                canonical_path: root.canonical_path.clone(),
                state: root.state,
                counts: root.counts,
                elapsed_ms: root.started.elapsed().as_millis() as u64,
                slow: root.slow,
                diagnostic: root.diagnostic.clone(),
            })
            .collect(),
        elapsed_ms: progress.started.elapsed().as_millis() as u64,
        slow: progress.slow,
        diagnostic: progress.diagnostic.clone(),
    }
}

pub(crate) fn parse_trigger(value: &str) -> ScanTrigger {
    match value {
        "manual" => ScanTrigger::Manual,
        _ => ScanTrigger::Onboarding,
    }
}

pub(crate) fn parse_report_state(value: &str) -> ScanReportState {
    if value == "complete" {
        ScanReportState::Complete
    } else {
        ScanReportState::Incomplete
    }
}

/// RFC3339 `YYYY-MM-DDTHH:MM:SSZ` → epoch millis (the ledger timestamps are
/// written by the recovery flow; unknown shapes yield 0 = fail closed).
pub(crate) fn rfc3339_to_epoch_millis(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let parse_digits = |range: std::ops::Range<usize>| -> Option<i64> {
        bytes
            .get(range.clone())
            .and_then(|slice| {
                let digits = slice.iter().all(u8::is_ascii_digit);
                digits
                    .then(|| std::str::from_utf8(&bytes[range.clone()]).ok())
                    .flatten()
            })
            .and_then(|text| text.parse().ok())
    };
    let year = parse_digits(0..4)?;
    let month = parse_digits(5..7)?;
    let day = parse_digits(8..10)?;
    let hour = parse_digits(11..13)?;
    let minute = parse_digits(14..16)?;
    let second = parse_digits(17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = crate::seams::clock::days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    Some((seconds as u64).saturating_mul(1_000))
}

fn write_gate_state_summary(state: &WriteGateState) -> String {
    match state {
        WriteGateState::Open(_) => "open".into(),
        WriteGateState::CatalogReadOnly { reason: _ } => "catalog read-only".into(),
        WriteGateState::Closed { reason } => format!("closed: {reason:?}"),
        WriteGateState::Recovery { .. } => "recovery".into(),
    }
}

pub(crate) fn seal_manifest(mut manifest: ScanReportManifest) -> ScanReportManifest {
    let value = serde_json::to_value(&manifest).expect("manifest serializes");
    manifest.integrity = Some(canonical_json_digest(&value));
    manifest
}
