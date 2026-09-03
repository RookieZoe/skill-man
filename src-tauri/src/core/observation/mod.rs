//! Observation and Scan Module (spec §4.10; ADR-0020): the single Core
//! Module owning Agent Detection, Startup Probe, Target-scoped Activation
//! Health Observation, the manual full Rescan / Evidence Store half, and
//! the shared `observation_page`/`report_page` paged reads. React,
//! onboarding, Agent Management and Adopt never compose their own
//! filesystem loops or generations.
//!
//! - Detection observes the nine Preset user-level roots: zero-write,
//!   single-flight, results live in memory; probe failures are
//!   `Unavailable`, never `Absent`.
//! - Startup Probe observes the configured Global Skills Roots and Agent
//!   Activation Targets (existence / readability / path identity) on
//!   startup, after Agent Configuration Apply and on explicit Retry; it is
//!   not a Rescan, writes nothing and never produces Report, Coverage or
//!   Adopt eligibility.
//! - Activation Health observes managed entry symlinks per Target group on
//!   the same scheduler; persistence is CAS-guarded (home_id, WriteGate
//!   generation, Agent generation, Target identity), failures keep the old
//!   observation as Unknown/Stale + diagnostic and never advance the
//!   filesystem-mutation generation nor make the Scan Report stale.
//! - Full Rescan runs one single-flight Run per Bound Home with the
//!   streaming Evidence Store (see `crate::core::scan`).

pub mod activation_health;
pub mod types;

use std::path::PathBuf;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use crate::core::agent_configuration::PresetRegistry;
use crate::core::scan::{
    CurrentReportView, ScanCoordinator, ScanCoordinatorSnapshot, ScanError, ScanRunSnapshot,
    ScanTrigger,
};
use crate::core::write_gate::WriteGate;
use crate::seams::activation_health::ActivationEntryFileSystem;
use crate::seams::activation_store::ActivationStore;
use crate::seams::agent_configuration_fs::{
    AgentConfigurationFileSystem, AgentConfigurationFileSystemError, AgentRootProbe,
};
use crate::seams::agent_configuration_store::AgentConfigurationStore;
use crate::seams::clock::Clock;

pub use self::types::{
    ActivationHealthCounts, ActivationHealthRow, ActivationHealthSnapshot, ObservationCursor,
    ObservationKind, ObservationPageError, ObservationPageRead, ObservationRow, ObservationStatus,
    StartupProbeRootCounts, StartupProbeRow, StartupProbeSnapshot, StartupProbeTargetCounts,
};

use self::activation_health::{
    DEFAULT_HEALTH_SLOW_MS, DEFAULT_UNRESPONSIVE_MS, HealthRunContext, HealthRunHandle,
    HealthSharedState,
};

pub const DEFAULT_PROBE_SLOW_MS: u64 = 1_000;

/// Closed per-root observation state. An unreadable or invalid root is
/// `Unavailable` with a diagnostic — never downgraded to `Absent`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootDetectionState {
    Present,
    Unavailable,
    Absent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootObservation {
    pub configured_path: PathBuf,
    pub state: RootDetectionState,
    /// Resolved directory identity when `Present`.
    pub canonical_path: Option<PathBuf>,
    /// Raw reason when `Unavailable`; never user copy.
    pub diagnostic: Option<String>,
}

/// Closed per-preset observation state. `Unknown` is the honest pre-run
/// state: the detector has not observed this preset yet, and absence of
/// evidence must never be rendered as absence of the agent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresetDetectionState {
    Present,
    Unavailable,
    Absent,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetObservation {
    pub preset_key: String,
    pub name: String,
    pub state: PresetDetectionState,
    pub roots: Vec<RootObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectionSnapshot {
    pub generation: u64,
    pub preset_observations: Vec<PresetObservation>,
}

/// Bounded summary shared as the query result and `observation://changed`
/// event payload (spec §4.10: the two are isomorphic). Every section is
/// bounded; detail is served by the paged read contracts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationAndScanSnapshot {
    pub home_id: Option<String>,
    pub write_gate_generation: u64,
    pub agent_configuration_generation: Option<u64>,
    pub detection: DetectionSnapshot,
    /// Startup Probe bounded summary (spec §4.10 `startup_probe`); `None`
    /// when the Agent Configuration is not readable (no observation).
    pub startup_probe: Option<StartupProbeSnapshot>,
    /// Activation Health bounded summary (spec §4.10 `activation_health`).
    pub activation_health: Option<ActivationHealthSnapshot>,
    /// The active/last Rescan Run (spec §4.10 `scan_run`).
    pub scan_run: Option<ScanRunSnapshot>,
    /// The bounded current Report view (spec §4.10 `current_report`).
    pub current_report: CurrentReportView,
}

/// Single-flight run bookkeeping: exactly one thread may probe; concurrent
/// triggers register a channel and reuse the Run's published result.
enum RunWaiters {
    Idle,
    Running(Vec<SyncSender<()>>),
}

struct DetectionShared {
    run: RunWaiters,
    snapshot: DetectionSnapshot,
}

struct ProbeShared {
    run: RunWaiters,
    snapshot: StartupProbeSnapshot,
    rows: Vec<types::StartupProbeRow>,
    /// At least one successful Observation run (pageable).
    initialized: bool,
}

impl ProbeShared {
    fn fresh() -> Self {
        Self {
            run: RunWaiters::Idle,
            snapshot: types::StartupProbeSnapshot {
                generation: 0,
                status: ObservationStatus::Unknown,
                root_counts: types::StartupProbeRootCounts::default(),
                target_counts: types::StartupProbeTargetCounts::default(),
                slow: false,
                diagnostic: None,
            },
            rows: Vec::new(),
            initialized: false,
        }
    }
}

/// Async observation run events (Activation Health transitions); the
/// payload is the same isomorphic snapshot the queries return.
pub trait ObservationEventObserver: Send + Sync {
    fn on_observation_event(&self, snapshot: &ObservationAndScanSnapshot);
}

pub struct ObservationService {
    filesystem: Arc<dyn AgentConfigurationFileSystem>,
    fs: Arc<dyn ActivationEntryFileSystem>,
    presets: PresetRegistry,
    write_gate: Arc<WriteGate>,
    agent_store: Arc<dyn AgentConfigurationStore>,
    activation_store: Arc<dyn ActivationStore>,
    clock: Arc<dyn Clock>,
    scan: Option<Arc<ScanCoordinator>>,
    detection: Arc<Mutex<DetectionShared>>,
    probe: Arc<Mutex<ProbeShared>>,
    health: Arc<Mutex<HealthSharedState>>,
    health_run: Mutex<Option<HealthRunHandle>>,
    observer: Arc<RwLock<Option<Arc<dyn ObservationEventObserver>>>>,
    probe_slow_ms: u64,
    health_slow_ms: u64,
    unresponsive_ms: u64,
}

impl ObservationService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        filesystem: Arc<dyn AgentConfigurationFileSystem>,
        fs: Arc<dyn ActivationEntryFileSystem>,
        presets: PresetRegistry,
        write_gate: Arc<WriteGate>,
        agent_store: Arc<dyn AgentConfigurationStore>,
        activation_store: Arc<dyn ActivationStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let detection = DetectionSnapshot {
            generation: 0,
            preset_observations: presets
                .presets()
                .iter()
                .map(|preset| PresetObservation {
                    preset_key: preset.preset_key.clone(),
                    name: preset.name.clone(),
                    state: PresetDetectionState::Unknown,
                    roots: Vec::new(),
                })
                .collect(),
        };
        Self {
            filesystem,
            fs,
            presets,
            write_gate,
            agent_store,
            activation_store,
            clock,
            scan: None,
            detection: Arc::new(Mutex::new(DetectionShared {
                run: RunWaiters::Idle,
                snapshot: detection,
            })),
            probe: Arc::new(Mutex::new(ProbeShared::fresh())),
            health: Arc::new(Mutex::new(HealthSharedState::fresh())),
            health_run: Mutex::new(None),
            observer: Arc::new(RwLock::new(None)),
            probe_slow_ms: DEFAULT_PROBE_SLOW_MS,
            health_slow_ms: DEFAULT_HEALTH_SLOW_MS,
            unresponsive_ms: DEFAULT_UNRESPONSIVE_MS,
        }
    }

    /// Test composition: shorten the Slow threshold for Startup Probe.
    pub fn with_probe_slow_ms(mut self, milliseconds: u64) -> Self {
        self.probe_slow_ms = milliseconds;
        self
    }

    /// Test composition: shorten the Slow threshold for Activation health.
    pub fn with_health_slow_ms(mut self, milliseconds: u64) -> Self {
        self.health_slow_ms = milliseconds;
        self
    }

    /// Test composition: shorten the zero-progress isolation window.
    pub fn with_unresponsive_ms(mut self, milliseconds: u64) -> Self {
        self.unresponsive_ms = milliseconds;
        self
    }

    /// Async observation events (Activation Health run transitions); the
    /// same channel the Scan Coordinator uses for Run progress.
    pub fn set_observation_observer(&self, observer: Arc<dyn ObservationEventObserver>) {
        *self
            .observer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(observer);
    }

    /// Attach the Observation and Scan Module's Rescan/Evidence Store half
    /// (spec §4.10): the same Service owns the shared Interface so React,
    /// onboarding, Agent Management and Adopt never compose their own
    /// filesystem loops or generations.
    pub fn with_scan(mut self, scan: Arc<ScanCoordinator>) -> Self {
        self.scan = Some(scan);
        self
    }

    /// Current in-memory view; never probes the filesystem and never
    /// blocks on a running observation (the health snapshot reads the
    /// last published generation state).
    pub fn snapshot(&self) -> ObservationAndScanSnapshot {
        self.ensure_health_initialized();
        let detection = self
            .detection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot
            .clone();
        let probe = self
            .probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let startup_probe = probe.initialized.then(|| probe.snapshot.clone());
        let health = self
            .health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let activation_health = health.initialized.then(|| ActivationHealthSnapshot {
            generation: health.generation,
            status: health.status,
            target_group_counts: health.counts(),
            slow: health.slow,
            diagnostic: health.diagnostic.clone(),
        });
        let scan = self.scan.as_ref().map(|coordinator| coordinator.snapshot());
        ObservationAndScanSnapshot {
            home_id: self.write_gate.bound_home().ok().map(|home| home.home_id.0),
            write_gate_generation: self.write_gate.generation(),
            agent_configuration_generation: self
                .agent_store
                .agent_configuration_snapshot()
                .ok()
                .map(|snapshot| snapshot.snapshot_version),
            detection,
            startup_probe,
            activation_health,
            scan_run: scan.as_ref().and_then(|snapshot| snapshot.run.clone()),
            current_report: scan.map(|snapshot| snapshot.current_report).unwrap_or(
                CurrentReportView {
                    summary: None,
                    freshness: crate::core::scan::ReportFreshness::Stale,
                    stale_reasons: vec![crate::core::scan::StaleReason::CrossStartup],
                },
            ),
        }
    }

    /// Start a full Rescan Run (single-flight; spec §4.10). The Run is
    /// started on the Scan Coordinator; the caller publishes
    /// `observation://changed` when the Run state advanced.
    pub fn start_rescan(
        &self,
        trigger: ScanTrigger,
    ) -> Result<ObservationAndScanSnapshot, ScanError> {
        let scan = self
            .scan
            .as_ref()
            .ok_or_else(|| ScanError::Internal("the Scan Coordinator is not attached".into()))?;
        let started: ScanCoordinatorSnapshot = scan.start_rescan(trigger)?;
        Ok(self.snapshot_with(started))
    }

    /// Coordinate cancellation of the active Run (spec §4.10).
    pub fn cancel_rescan(&self, run_id: &str) -> Result<ObservationAndScanSnapshot, ScanError> {
        let scan = self
            .scan
            .as_ref()
            .ok_or_else(|| ScanError::Internal("the Scan Coordinator is not attached".into()))?;
        let cancelled: ScanCoordinatorSnapshot = scan.cancel_rescan(run_id)?;
        Ok(self.snapshot_with(cancelled))
    }

    /// The unique paged Report read contract (spec §4.10 `report_page`):
    /// the scan module owns the filesystem loop and generation, React never
    /// composes its own.
    pub fn report_page(
        &self,
        cursor: crate::seams::scan_evidence_store::ScanReportCursor,
        limit: usize,
    ) -> Result<crate::seams::scan_evidence_store::ScanReportPageRead, ScanError> {
        let scan = self
            .scan
            .as_ref()
            .ok_or_else(|| ScanError::Internal("the Scan Coordinator is not attached".into()))?;
        scan.report_page(cursor, limit)
    }

    /// The unique paged observation read contract (spec §4.10
    /// `observation_page`): rows of one generation only; a cursor whose
    /// generation is no longer current returns typed stale, a kind that was
    /// never observable returns not-found — never a fallback to another
    /// observation generation.
    pub fn observation_page(
        &self,
        kind: ObservationKind,
        generation: u64,
        cursor: ObservationCursor,
        limit: usize,
    ) -> Result<ObservationPageRead, ObservationPageError> {
        self.ensure_health_initialized();
        let rows: Vec<ObservationRow> = match kind {
            ObservationKind::StartupProbe => {
                let probe = self
                    .probe
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !probe.initialized {
                    return Err(ObservationPageError::NotFound);
                }
                if probe.snapshot.generation != generation {
                    return Err(ObservationPageError::Stale {
                        current_generation: probe.snapshot.generation,
                    });
                }
                probe
                    .rows
                    .iter()
                    .cloned()
                    .map(ObservationRow::StartupProbe)
                    .collect()
            }
            ObservationKind::ActivationHealth => {
                let health = self
                    .health
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !health.initialized {
                    return Err(ObservationPageError::NotFound);
                }
                if health.generation != generation {
                    return Err(ObservationPageError::Stale {
                        current_generation: health.generation,
                    });
                }
                health
                    .flat_rows()
                    .into_iter()
                    .map(ObservationRow::ActivationHealth)
                    .collect()
            }
        };
        Ok(page_rows(&rows, cursor.offset, limit))
    }

    /// Lazy cross-startup load of the persisted Activation Health rows:
    /// the first screen shows the old observations as `Stale` until a run
    /// re-observes them. Read-only; a closed store leaves the section
    /// absent (fail-closed, no guessed state).
    fn ensure_health_initialized(&self) {
        let mut health = self
            .health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if health.initialized {
            return;
        }
        if let Ok(stored) = self.activation_store.activation_observations() {
            health.groups = activation_health::initial_groups(&stored);
            health.status = ObservationStatus::Stale;
            health.initialized = true;
        }
    }

    fn snapshot_with(&self, scan: ScanCoordinatorSnapshot) -> ObservationAndScanSnapshot {
        let mut snapshot = self.snapshot();
        snapshot.scan_run = scan.run;
        snapshot.current_report = scan.current_report;
        snapshot
    }

    /// Run or join a Detection Run (single-flight, ADR-0020): a trigger
    /// while a Run is active reuses its published result instead of probing
    /// again. The result is published in memory and returned; the caller
    /// publishes the `observation://changed` payload when the generation
    /// advanced.
    pub fn refresh_detection(&self) -> ObservationAndScanSnapshot {
        let (owned_run, joined) = {
            let mut state = self
                .detection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &mut state.run {
                RunWaiters::Running(waiters) => {
                    let (sender, receiver) = sync_channel(1);
                    waiters.push(sender);
                    (false, Some(receiver))
                }
                RunWaiters::Idle => {
                    state.run = RunWaiters::Running(Vec::new());
                    (true, None)
                }
            }
        };
        if owned_run {
            let published = {
                let generation = self
                    .detection
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .snapshot
                    .generation
                    .wrapping_add(1);
                self.probe_detection(generation)
            };
            let mut state = self
                .detection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.snapshot = published.clone();
            if let RunWaiters::Running(waiters) = &mut state.run {
                for waiter in waiters.drain(..) {
                    let _ = waiter.send(());
                }
            }
            state.run = RunWaiters::Idle;
        } else {
            joined
                .expect("a joined Run always has a receiver")
                .recv()
                .expect("an active Detection Run always publishes");
        }
        self.snapshot()
    }

    /// Run or join one Startup Probe observation (single-flight): startup,
    /// Agent Configuration Apply and explicit Retry all route here. The
    /// probe only reads configured Root/Target existence/readability/path
    /// identity — it never triggers a Rescan nor produces Report, Coverage
    /// or Adopt eligibility.
    pub fn refresh_startup_probe(&self) -> ObservationAndScanSnapshot {
        let (owned_run, joined) = {
            let mut state = self
                .probe
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &mut state.run {
                RunWaiters::Running(waiters) => {
                    let (sender, receiver) = sync_channel(1);
                    waiters.push(sender);
                    (false, Some(receiver))
                }
                RunWaiters::Idle => {
                    state.run = RunWaiters::Running(Vec::new());
                    (true, None)
                }
            }
        };
        if owned_run {
            let started = Instant::now();
            let outcome = {
                let generation = self
                    .probe
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .snapshot
                    .generation
                    .wrapping_add(1);
                probe_run::probe(self, generation, started)
            };
            let mut state = self
                .probe
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match outcome {
                Ok(published) => {
                    state.snapshot = published.snapshot;
                    state.rows = published.rows;
                    state.initialized = true;
                }
                Err(diagnostic) => {
                    // Keep the previous view; a failed attempt never erases
                    // observed evidence (fail-closed, no guessed state).
                    if state.initialized {
                        state.snapshot.diagnostic = Some(diagnostic);
                    }
                }
            }
            if let RunWaiters::Running(waiters) = &mut state.run {
                for waiter in waiters.drain(..) {
                    let _ = waiter.send(());
                }
            }
            state.run = RunWaiters::Idle;
        } else {
            joined
                .expect("a joined Run always has a receiver")
                .recv()
                .expect("an active Startup Probe run always publishes");
        }
        self.snapshot()
    }

    /// Trigger a Target-scoped Activation Health observation run (spec
    /// §4.10 / ADR-0020): startup, successful Enable/Disable/Repair, Target
    /// configuration change and explicit Retry schedule only the affected
    /// Targets; an active run is superseded (never joined) so an
    /// enable/repair trigger is applied immediately. The run publishes
    /// events through the observation observer; this method returns the
    /// current snapshot with the new generation marked `Checking`.
    pub fn refresh_activation_health(
        &self,
        targets: Option<&[String]>,
    ) -> ObservationAndScanSnapshot {
        let in_scope = targets.unwrap_or(&[]).to_vec();
        {
            let mut run_slot = self
                .health_run
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(previous) = run_slot.take() {
                previous.request_stop();
            }
        }
        let emit = self.health_emit_closure();
        let handle = activation_health::trigger(
            &self.health_run_context(),
            self.health.clone(),
            in_scope,
            emit,
            self.unresponsive_ms,
            self.health_slow_ms,
        );
        {
            let mut run_slot = self
                .health_run
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *run_slot = Some(handle);
        }
        self.snapshot()
    }

    fn health_run_context(&self) -> HealthRunContext {
        HealthRunContext {
            activation_store: self.activation_store.clone(),
            agent_store: self.agent_store.clone(),
            config_fs: self.filesystem.clone(),
            filesystem: self.fs.clone(),
            write_gate: self.write_gate.clone(),
            clock: self.clock.clone(),
        }
    }

    /// The run observer callback with the 250 ms progress throttle; the
    /// payload is the same isomorphic snapshot the queries return.
    fn health_emit_closure(&self) -> Arc<dyn Fn(bool) + Send + Sync> {
        let write_gate = self.write_gate.clone();
        let agent_store = self.agent_store.clone();
        let scan = self.scan.clone();
        let detection = self.detection.clone();
        let probe = self.probe.clone();
        let health = self.health.clone();
        let observer = self.observer.clone();
        let throttle = Arc::new(Mutex::new(None::<Instant>));
        Arc::new(move |force: bool| {
            let now = Instant::now();
            {
                let mut last = throttle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !force
                    && last
                        .as_ref()
                        .is_some_and(|moment| now.duration_since(*moment).as_millis() < 250)
                {
                    return;
                }
                *last = Some(now);
            }
            let snapshot = build_snapshot(
                &write_gate,
                &agent_store,
                &scan,
                &detection,
                &probe,
                &health,
            );
            if let Some(observer) = observer
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                observer.on_observation_event(&snapshot);
            }
        })
    }

    fn probe_detection(&self, generation: u64) -> DetectionSnapshot {
        let preset_observations = self
            .presets
            .presets()
            .iter()
            .map(|preset| {
                let roots = preset
                    .roots
                    .iter()
                    .map(|configured_path| self.probe_root(configured_path))
                    .collect::<Vec<_>>();
                let state = if roots
                    .iter()
                    .any(|root| root.state == RootDetectionState::Present)
                {
                    PresetDetectionState::Present
                } else if roots
                    .iter()
                    .any(|root| root.state == RootDetectionState::Unavailable)
                {
                    PresetDetectionState::Unavailable
                } else {
                    PresetDetectionState::Absent
                };
                PresetObservation {
                    preset_key: preset.preset_key.clone(),
                    name: preset.name.clone(),
                    state,
                    roots,
                }
            })
            .collect();
        DetectionSnapshot {
            generation,
            preset_observations,
        }
    }

    fn probe_root(&self, configured_path: &std::path::Path) -> RootObservation {
        let outcome = self.filesystem.probe_root(configured_path);
        match outcome {
            Ok(AgentRootProbe::Present { canonical_path }) => RootObservation {
                configured_path: configured_path.to_path_buf(),
                state: RootDetectionState::Present,
                canonical_path: Some(canonical_path),
                diagnostic: None,
            },
            Ok(AgentRootProbe::Unavailable { diagnostic }) => RootObservation {
                configured_path: configured_path.to_path_buf(),
                state: RootDetectionState::Unavailable,
                canonical_path: None,
                diagnostic: Some(diagnostic),
            },
            Ok(AgentRootProbe::Absent) => RootObservation {
                configured_path: configured_path.to_path_buf(),
                state: RootDetectionState::Absent,
                canonical_path: None,
                diagnostic: None,
            },
            Err(error) => RootObservation {
                configured_path: configured_path.to_path_buf(),
                state: RootDetectionState::Unavailable,
                canonical_path: None,
                diagnostic: Some(agent_root_probe_error(&error)),
            },
        }
    }
}

fn build_snapshot(
    write_gate: &WriteGate,
    agent_store: &Arc<dyn AgentConfigurationStore>,
    scan: &Option<Arc<ScanCoordinator>>,
    detection: &Arc<Mutex<DetectionShared>>,
    probe: &Arc<Mutex<ProbeShared>>,
    health: &Arc<Mutex<HealthSharedState>>,
) -> ObservationAndScanSnapshot {
    let detection_state = detection
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let probe_state = probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let health_state = health
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let startup_probe = probe_state
        .initialized
        .then(|| probe_state.snapshot.clone());
    let activation_health = health_state.initialized.then(|| ActivationHealthSnapshot {
        generation: health_state.generation,
        status: health_state.status,
        target_group_counts: health_state.counts(),
        slow: health_state.slow,
        diagnostic: health_state.diagnostic.clone(),
    });
    let scan_state = scan.as_ref().map(|coordinator| coordinator.snapshot());
    ObservationAndScanSnapshot {
        home_id: write_gate.bound_home().ok().map(|home| home.home_id.0),
        write_gate_generation: write_gate.generation(),
        agent_configuration_generation: agent_store
            .agent_configuration_snapshot()
            .ok()
            .map(|snapshot| snapshot.snapshot_version),
        detection: detection_state.snapshot.clone(),
        startup_probe,
        activation_health,
        scan_run: scan_state
            .as_ref()
            .and_then(|snapshot| snapshot.run.clone()),
        current_report: scan_state
            .map(|snapshot| snapshot.current_report)
            .unwrap_or(CurrentReportView {
                summary: None,
                freshness: crate::core::scan::ReportFreshness::Stale,
                stale_reasons: vec![crate::core::scan::StaleReason::CrossStartup],
            }),
    }
}

fn page_rows(rows: &[ObservationRow], offset: u64, limit: usize) -> ObservationPageRead {
    let start = (offset as usize).min(rows.len());
    let end = start.saturating_add(limit).min(rows.len());
    let page = rows[start..end].to_vec();
    ObservationPageRead {
        rows: page,
        next_offset: if end < rows.len() {
            Some(end as u64)
        } else {
            None
        },
    }
}

fn agent_root_probe_error(error: &AgentConfigurationFileSystemError) -> String {
    match error {
        AgentConfigurationFileSystemError::InvalidPath => "invalid_path".to_owned(),
        AgentConfigurationFileSystemError::NotDirectory => "not_a_directory".to_owned(),
        AgentConfigurationFileSystemError::Unavailable(message) => message.clone(),
        other => other.to_string(),
    }
}

/// Startup Probe internals: configured Roots + Activation Targets. Kept in
/// this module so the probe never composes its own config reads.
mod probe_run {
    use super::*;
    use crate::core::agent_configuration::AgentRootRole;
    use crate::seams::agent_configuration_store::AgentConfigurationStoreSnapshot;

    pub(super) struct ProbeOutcome {
        pub(super) snapshot: StartupProbeSnapshot,
        pub(super) rows: Vec<types::StartupProbeRow>,
    }

    /// Read the Agent Configuration snapshot and probe every configured
    /// Root (and Target) sequentially; `slow` is set once the run crosses
    /// the threshold — never a truncation or downgrade. An unreadable
    /// configuration is an error: the previous view is kept untouched.
    pub(super) fn probe(
        service: &ObservationService,
        generation: u64,
        started: std::time::Instant,
    ) -> Result<ProbeOutcome, String> {
        let snapshot = service
            .agent_store
            .agent_configuration_snapshot()
            .map_err(|error| format!("agent configuration unavailable: {error}"))?;
        let (rows, root_counts, target_counts, slow) = probe_snapshot(service, &snapshot, started);
        Ok(ProbeOutcome {
            snapshot: StartupProbeSnapshot {
                generation,
                status: ObservationStatus::Observed,
                root_counts,
                target_counts,
                slow,
                diagnostic: None,
            },
            rows,
        })
    }

    fn probe_snapshot(
        service: &ObservationService,
        snapshot: &AgentConfigurationStoreSnapshot,
        started: std::time::Instant,
    ) -> (
        Vec<types::StartupProbeRow>,
        types::StartupProbeRootCounts,
        types::StartupProbeTargetCounts,
        bool,
    ) {
        let target_ids = snapshot
            .configurations
            .iter()
            .flat_map(|configuration| {
                configuration
                    .memberships
                    .iter()
                    .filter(|membership| membership.role == AgentRootRole::ActivationTarget)
                    .map(|membership| membership.root_id.clone())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut rows = Vec::new();
        let mut root_counts = types::StartupProbeRootCounts::default();
        let mut target_counts = types::StartupProbeTargetCounts::default();
        let mut slow = false;
        for root in &snapshot.roots {
            let observation = service.probe_root(&root.configured_path);
            let state = observation.state;
            let is_target = target_ids.contains(&root.root_id);
            if is_target {
                target_counts.total += 1;
                match state {
                    RootDetectionState::Present => target_counts.present += 1,
                    RootDetectionState::Unavailable => target_counts.unavailable += 1,
                    RootDetectionState::Absent => target_counts.absent += 1,
                }
            } else {
                root_counts.total += 1;
                match state {
                    RootDetectionState::Present => root_counts.present += 1,
                    RootDetectionState::Unavailable => root_counts.unavailable += 1,
                    RootDetectionState::Absent => root_counts.absent += 1,
                }
            }
            rows.push(types::StartupProbeRow {
                configured_path: root.configured_path.clone(),
                path_identity_key: root.path_identity_key.clone(),
                canonical_path: observation.canonical_path.clone(),
                state: observation.state,
                diagnostic: observation.diagnostic.clone(),
                is_target,
            });
            if started.elapsed().as_millis() as u64 >= service.probe_slow_ms {
                slow = true;
            }
        }
        rows.sort_by(|left, right| {
            left.path_identity_key
                .cmp(&right.path_identity_key)
                .then_with(|| left.configured_path.cmp(&right.configured_path))
        });
        (rows, root_counts, target_counts, slow)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use super::*;
    use crate::core::agent_configuration::{AgentConfigurationOrigin, AgentPreset, AgentRootRole};
    use crate::core::domain::{ActivationObservedState, SkillId};
    use crate::core::home::BoundHome;
    use crate::core::write_gate::{ReadOnlyReason, WriteGate, WriteGateState};
    use crate::seams::activation_health::ActivationEntryFileSystem;
    use crate::seams::activation_store::{
        ActivationObservation, ActivationStore, ActivationStoreError, DesiredActivation,
        StoredActivationObservation,
    };
    use crate::seams::agent_configuration_fs::{
        AgentConfigurationFileSystem, AgentConfigurationFileSystemError, AgentRootInspection,
        AgentRootProbe, CreatedAgentTargetDirectory,
    };
    use crate::seams::agent_configuration_store::{
        AgentConfigurationStore, AgentConfigurationStoreChange, AgentConfigurationStoreError,
        AgentConfigurationStoreSnapshot, RecentProjectFolder, StoredAgentConfiguration,
        StoredAgentRootMembership, StoredGlobalSkillRoot,
    };
    use crate::seams::clock::Clock;
    use crate::seams::filesystem::{ActivationEntrySnapshot, FileSystemError};

    // -- Test doubles ------------------------------------------------------

    #[derive(Clone)]
    struct StubAgentFs {
        outcomes: std::collections::HashMap<PathBuf, AgentRootProbe>,
        calls: Arc<AtomicUsize>,
        /// When set, every probe of a matching path blocks until released.
        block: Arc<Mutex<std::collections::HashMap<PathBuf, std::sync::mpsc::Receiver<()>>>>,
    }

    impl StubAgentFs {
        fn new(outcomes: Vec<(&str, AgentRootProbe)>) -> Self {
            Self {
                outcomes: outcomes
                    .into_iter()
                    .map(|(path, outcome)| (PathBuf::from(path), outcome))
                    .collect(),
                calls: Arc::new(AtomicUsize::new(0)),
                block: Arc::new(Mutex::new(std::collections::HashMap::new())),
            }
        }
    }

    impl AgentConfigurationFileSystem for StubAgentFs {
        fn inspect_root(
            &self,
            _configured_path: &Path,
        ) -> Result<AgentRootInspection, AgentConfigurationFileSystemError> {
            panic!("observation never inspects roots")
        }

        fn probe_root(
            &self,
            configured_path: &Path,
        ) -> Result<AgentRootProbe, AgentConfigurationFileSystemError> {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if let Some(receiver) = self
                .block
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(configured_path)
            {
                let _ = receiver.recv();
            }
            Ok(self
                .outcomes
                .get(configured_path)
                .cloned()
                .unwrap_or(AgentRootProbe::Absent))
        }

        fn create_target(
            &self,
            _planned: &AgentRootInspection,
        ) -> Result<CreatedAgentTargetDirectory, AgentConfigurationFileSystemError> {
            panic!("observation never creates targets")
        }

        fn rollback_created_target(
            &self,
            _receipt: &CreatedAgentTargetDirectory,
        ) -> Result<(), AgentConfigurationFileSystemError> {
            panic!("observation never creates targets")
        }

        fn random_bytes(
            &self,
            _buffer: &mut [u8],
        ) -> Result<(), AgentConfigurationFileSystemError> {
            panic!("observation never draws entropy")
        }
    }

    #[derive(Clone)]
    struct StubFs {
        /// entry_path -> snapshot or error marker.
        entry_outcomes: std::collections::HashMap<PathBuf, ActivationEntrySnapshot>,
        entry_errors: std::collections::HashMap<PathBuf, String>,
        readable: std::collections::HashMap<PathBuf, bool>,
        entry_calls: Arc<AtomicUsize>,
        /// Paths whose check blocks until released (target isolation tests).
        block: Arc<Mutex<std::collections::HashMap<PathBuf, std::sync::mpsc::Receiver<()>>>>,
        started: Arc<Mutex<Option<std::sync::mpsc::SyncSender<()>>>>,
    }

    impl StubFs {
        fn new() -> Self {
            Self {
                entry_outcomes: std::collections::HashMap::new(),
                entry_errors: std::collections::HashMap::new(),
                readable: std::collections::HashMap::new(),
                entry_calls: Arc::new(AtomicUsize::new(0)),
                block: Arc::new(Mutex::new(std::collections::HashMap::new())),
                started: Arc::new(Mutex::new(None)),
            }
        }

        fn entries(&self, outcomes: Vec<(&str, ActivationEntrySnapshot)>) -> Self {
            let mut stub = self.clone();
            stub.entry_outcomes = outcomes
                .into_iter()
                .map(|(path, outcome)| (PathBuf::from(path), outcome))
                .collect();
            stub
        }
    }

    impl ActivationEntryFileSystem for StubFs {
        fn activation_snapshot(
            &self,
            path: &Path,
        ) -> Result<ActivationEntrySnapshot, FileSystemError> {
            self.entry_calls.fetch_add(1, AtomicOrdering::SeqCst);
            // Drop the block-map guard before waiting: the receiver blocks
            // worker progress, never the stub's lock.
            let receiver = self
                .block
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(path);
            if let Some(receiver) = receiver {
                if let Some(started) = self
                    .started
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    let _ = started.send(());
                }
                let _ = receiver.recv();
            }
            if let Some(_error) = self.entry_errors.get(path) {
                return Err(FileSystemError::Io {
                    operation: "read",
                    path: path.to_path_buf(),
                    source: std::io::Error::other("injected health failure"),
                });
            }
            Ok(self
                .entry_outcomes
                .get(path)
                .cloned()
                .unwrap_or(ActivationEntrySnapshot::Missing))
        }

        fn skill_directory_is_readable(&self, path: &Path) -> Result<bool, FileSystemError> {
            Ok(self.readable.get(path).copied().unwrap_or(false))
        }
    }

    struct StubAgentStoreInner {
        snapshot_version: u64,
        roots: Vec<StoredGlobalSkillRoot>,
        configurations: Vec<StoredAgentConfiguration>,
        fail: Option<String>,
    }

    #[derive(Clone)]
    struct StubAgentStore {
        inner: Arc<Mutex<StubAgentStoreInner>>,
    }

    impl StubAgentStore {
        fn new(snapshot_version: u64) -> Self {
            Self {
                inner: Arc::new(Mutex::new(StubAgentStoreInner {
                    snapshot_version,
                    roots: Vec::new(),
                    configurations: Vec::new(),
                    fail: None,
                })),
            }
        }

        fn with(&self, mutate: impl FnOnce(&mut StubAgentStoreInner)) {
            mutate(
                &mut self
                    .inner
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }
    }

    impl AgentConfigurationStore for StubAgentStore {
        fn agent_configuration_snapshot(
            &self,
        ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError> {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(message) = &inner.fail {
                return Err(AgentConfigurationStoreError::Unavailable(message.clone()));
            }
            Ok(AgentConfigurationStoreSnapshot {
                snapshot_version: inner.snapshot_version,
                configurations: inner.configurations.clone(),
                roots: inner.roots.clone(),
            })
        }

        fn apply_agent_configuration_change(
            &self,
            _expected_snapshot_version: u64,
            _change: AgentConfigurationStoreChange,
        ) -> Result<u64, AgentConfigurationStoreError> {
            unreachable!("observation never applies configuration changes")
        }

        fn list_recent_project_folders(
            &self,
        ) -> Result<Vec<RecentProjectFolder>, AgentConfigurationStoreError> {
            unreachable!("observation never reads project folders")
        }

        fn record_recent_project_folder(
            &self,
            _folder: RecentProjectFolder,
        ) -> Result<(), AgentConfigurationStoreError> {
            unreachable!("observation never records project folders")
        }

        fn clear_recent_project_folders(&self) -> Result<(), AgentConfigurationStoreError> {
            unreachable!("observation never clears project folders")
        }
    }

    /// Records every persisted observation batch.
    #[derive(Clone)]
    struct StubActivationStore {
        stored: Vec<StoredActivationObservation>,
        recorded: Arc<Mutex<Vec<Vec<ActivationObservation>>>>,
        fail: Arc<Mutex<Option<String>>>,
    }

    impl StubActivationStore {
        fn new() -> Self {
            Self {
                stored: Vec::new(),
                recorded: Arc::new(Mutex::new(Vec::new())),
                fail: Arc::new(Mutex::new(None)),
            }
        }

        fn with_rows(&mut self, rows: Vec<StoredActivationObservation>) -> Self {
            self.stored = rows;
            self.clone()
        }
    }

    impl ActivationStore for StubActivationStore {
        fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
            Ok(self
                .stored
                .iter()
                .map(|row| DesiredActivation {
                    skill_id: row.skill_id.clone(),
                    target_root_id: row.target_root_id.clone(),
                    expected_entry_path: row.expected_entry_path.clone(),
                    expected_target_path: row.expected_target_path.clone(),
                })
                .collect())
        }

        fn activation_observations(
            &self,
        ) -> Result<Vec<StoredActivationObservation>, ActivationStoreError> {
            if let Some(message) = self
                .fail
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                return Err(ActivationStoreError::Unavailable(message));
            }
            Ok(self.stored.clone())
        }

        fn record_observations(
            &self,
            observations: &[ActivationObservation],
        ) -> Result<u64, ActivationStoreError> {
            if let Some(message) = self
                .fail
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                return Err(ActivationStoreError::Unavailable(message));
            }
            self.recorded
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(observations.to_vec());
            Ok(1)
        }
    }

    struct StubClock;

    impl Clock for StubClock {
        fn unix_epoch_nanos(&self) -> u128 {
            1_800_000_000_000_000_000
        }

        fn monotonic_millis(&self) -> u128 {
            1_800_000_000_000
        }
    }

    fn test_preset(key: &str, name: &str, roots: Vec<&str>) -> AgentPreset {
        AgentPreset {
            preset_key: key.into(),
            name: name.into(),
            compatibility: crate::core::domain::Compatibility::Verified,
            roots: roots.into_iter().map(PathBuf::from).collect(),
            activation_target: PathBuf::from("~/.target"),
            project_skills_dir: PathBuf::from(".skills"),
        }
    }

    fn test_presets() -> PresetRegistry {
        PresetRegistry::from_presets(vec![
            test_preset("alpha", "Alpha", vec!["~/.alpha/skills"]),
            test_preset("beta", "Beta", vec!["~/.beta/skills"]),
        ])
    }

    fn bound_gate() -> Arc<WriteGate> {
        Arc::new(WriteGate::new(WriteGateState::Open(BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            PathBuf::from("/tmp/skill-man-home"),
        ))))
    }

    fn root(root_id: &str, path: &str, role: AgentRootRole) -> StoredGlobalSkillRoot {
        let mut root = StoredGlobalSkillRoot {
            root_id: root_id.into(),
            configured_path: PathBuf::from(path),
            path_identity_key: crate::core::domain::configured_path_identity_key(path),
            consumer_agent_ids: Vec::new(),
            activation_skill_ids: Vec::new(),
        };
        if role == AgentRootRole::ActivationTarget {
            root.consumer_agent_ids.push("agent-1".into());
        }
        root
    }

    fn stored_row(
        skill: &str,
        target: &str,
        entry: &str,
        target_path: &str,
        observed: Option<ActivationObservedState>,
        checked_at_ms: Option<u64>,
    ) -> StoredActivationObservation {
        StoredActivationObservation {
            skill_id: SkillId(skill.into()),
            target_root_id: target.into(),
            expected_entry_path: PathBuf::from(entry),
            expected_target_path: PathBuf::from(target_path),
            observed_state: observed,
            last_checked_at_ms: checked_at_ms,
        }
    }

    fn harness_with_gate(
        gate: Arc<WriteGate>,
        agent_fs: StubAgentFs,
        fs: StubFs,
        agent_store: StubAgentStore,
        activation_store: StubActivationStore,
    ) -> (
        ObservationService,
        Arc<StubAgentFs>,
        Arc<StubFs>,
        Arc<StubActivationStore>,
        Arc<WriteGate>,
    ) {
        let agent_fs = Arc::new(agent_fs);
        let fs = Arc::new(fs);
        let activation_store = Arc::new(activation_store);
        let gate = gate.clone();
        let service = ObservationService::new(
            agent_fs.clone(),
            fs.clone(),
            test_presets(),
            gate.clone(),
            Arc::new(agent_store),
            activation_store.clone(),
            Arc::new(StubClock),
        );
        (service, agent_fs, fs, activation_store, gate)
    }

    fn harness(
        agent_fs: StubAgentFs,
        fs: StubFs,
        agent_store: StubAgentStore,
        activation_store: StubActivationStore,
    ) -> (
        ObservationService,
        Arc<StubAgentFs>,
        Arc<StubFs>,
        Arc<StubActivationStore>,
    ) {
        let (service, agent_fs, fs, activation_store, _gate) =
            harness_with_gate(bound_gate(), agent_fs, fs, agent_store, activation_store);
        (service, agent_fs, fs, activation_store)
    }

    /// Poll until the health run leaves `Checking` (bounded).
    fn wait_health(service: &ObservationService, timeout_ms: u64) -> ObservationAndScanSnapshot {
        let deadline = Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            let snapshot = service.snapshot();
            if let Some(health) = &snapshot.activation_health {
                if health.status == ObservationStatus::Observed {
                    return snapshot;
                }
            }
            if Instant::now() >= deadline {
                panic!(
                    "health run did not finish within {timeout_ms} ms: {:?}",
                    snapshot.activation_health
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // -- Startup Probe ------------------------------------------------------

    #[test]
    fn fresh_snapshot_has_no_observation_sections() {
        let (service, agent_fs, _fs, _activation) = harness(
            StubAgentFs::new(vec![]),
            StubFs::new(),
            StubAgentStore::new(7),
            StubActivationStore::new(),
        );
        let snapshot = service.snapshot();
        assert_eq!(snapshot.startup_probe, None);
        // A readable (empty) Catalog is observable: Stale with zero rows.
        let health = snapshot.activation_health.expect("readable empty store");
        assert_eq!(health.status, ObservationStatus::Stale);
        assert_eq!(health.target_group_counts.target_groups, 0);
        assert_eq!(snapshot.detection.generation, 0);
        assert_eq!(agent_fs.calls.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(
            snapshot.home_id.as_deref(),
            Some("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab")
        );
    }

    #[test]
    fn startup_probe_observes_roots_targets_and_never_writes() {
        let agent_store = StubAgentStore::new(3);
        agent_store.with(|s| {
            s.roots = vec![
                root("r-scan", "/Users/me/skills", AgentRootRole::ScanOnly),
                root(
                    "r-target",
                    "/Users/me/agents/skills",
                    AgentRootRole::ActivationTarget,
                ),
            ]
        });
        agent_store.with(|s| {
            s.configurations = vec![StoredAgentConfiguration {
                agent_id: "agent-1".into(),
                origin: AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: "Mine".into(),
                name_identity_key: "mine".into(),
                compatibility: crate::core::domain::Compatibility::Unknown,
                project_skills_dir: None,
                created_at: "t".into(),
                updated_at: "t".into(),
                memberships: vec![StoredAgentRootMembership {
                    root_id: "r-target".into(),
                    role: AgentRootRole::ActivationTarget,
                }],
            }]
        });
        let (service, _agent_fs, _fs, activation_store) = harness(
            StubAgentFs::new(vec![
                (
                    "/Users/me/skills",
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/skills"),
                    },
                ),
                ("/Users/me/agents/skills", AgentRootProbe::Absent),
            ]),
            StubFs::new(),
            agent_store,
            StubActivationStore::new(),
        );
        let snapshot = service.refresh_startup_probe();
        let probe = snapshot.startup_probe.expect("probe ran");
        assert_eq!(probe.generation, 1);
        assert_eq!(probe.status, ObservationStatus::Observed);
        assert_eq!(probe.root_counts.total, 1);
        assert_eq!(probe.root_counts.present, 1);
        assert_eq!(probe.target_counts.total, 1);
        assert_eq!(probe.target_counts.absent, 1);
        assert!(!probe.slow);
        assert!(snapshot.scan_run.is_none(), "probe never starts a Rescan");
        assert_eq!(
            activation_store.recorded.lock().unwrap().len(),
            0,
            "probe never persists"
        );
        let rows = service
            .observation_page(
                ObservationKind::StartupProbe,
                1,
                ObservationCursor { offset: 0 },
                64,
            )
            .expect("page read");
        assert_eq!(rows.rows.len(), 2);
        assert_eq!(rows.next_offset, None);
        let first = match &rows.rows[0] {
            ObservationRow::StartupProbe(row) => row,
            _ => panic!("expected probe row"),
        };
        assert_eq!(
            first.configured_path,
            PathBuf::from("/Users/me/agents/skills")
        );
        assert!(first.is_target);
        assert_eq!(first.state, RootDetectionState::Absent);
    }

    #[test]
    fn startup_probe_slow_is_flagged_but_not_truncated() {
        let agent_store = StubAgentStore::new(1);
        agent_store
            .with(|s| s.roots = vec![root("r1", "/Users/me/skills", AgentRootRole::ScanOnly)]);
        let (service, _agent_fs, _fs, _activation) = harness(
            StubAgentFs::new(vec![(
                "/Users/me/skills",
                AgentRootProbe::Present {
                    canonical_path: PathBuf::from("/Users/me/skills"),
                },
            )]),
            StubFs::new(),
            agent_store,
            StubActivationStore::new(),
        );
        let service = service.with_probe_slow_ms(0);
        let snapshot = service.refresh_startup_probe();
        let probe = snapshot.startup_probe.unwrap();
        assert!(probe.slow);
        assert_eq!(probe.root_counts.present, 1, "slow never truncates results");
    }

    #[test]
    fn startup_probe_is_single_flight_and_joins() {
        let agent_store = StubAgentStore::new(1);
        agent_store
            .with(|s| s.roots = vec![root("r1", "/Users/me/skills", AgentRootRole::ScanOnly)]);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let agent_fs = StubAgentFs::new(vec![(
            "/Users/me/skills",
            AgentRootProbe::Present {
                canonical_path: PathBuf::from("/Users/me/skills"),
            },
        )]);
        agent_fs
            .block
            .lock()
            .unwrap()
            .insert(PathBuf::from("/Users/me/skills"), release_rx);
        let (service, agent_fs, _fs, _activation) = harness(
            agent_fs,
            StubFs::new(),
            agent_store,
            StubActivationStore::new(),
        );
        let service = Arc::new(service);
        let first = {
            let service = service.clone();
            std::thread::spawn(move || service.refresh_startup_probe())
        };
        // The probe is blocked on the gated root; wait until it entered.
        while agent_fs.calls.load(AtomicOrdering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        let second = {
            let service = service.clone();
            std::thread::spawn(move || service.refresh_startup_probe())
        };
        std::thread::sleep(std::time::Duration::from_millis(30));
        release_tx.send(()).unwrap();
        let first_snapshot = first.join().unwrap();
        let second_snapshot = second.join().unwrap();
        assert_eq!(
            first_snapshot.startup_probe.as_ref().unwrap().generation,
            second_snapshot.startup_probe.as_ref().unwrap().generation
        );
        assert_eq!(
            service
                .snapshot()
                .startup_probe
                .as_ref()
                .unwrap()
                .generation,
            1
        );
    }

    #[test]
    fn probe_keeps_previous_view_when_configuration_becomes_unavailable() {
        let agent_store = StubAgentStore::new(1);
        agent_store
            .with(|s| s.roots = vec![root("r1", "/Users/me/skills", AgentRootRole::ScanOnly)]);
        let (service, _agent_fs, _fs, _activation) = harness(
            StubAgentFs::new(vec![(
                "/Users/me/skills",
                AgentRootProbe::Present {
                    canonical_path: PathBuf::from("/Users/me/skills"),
                },
            )]),
            StubFs::new(),
            agent_store.clone(),
            StubActivationStore::new(),
        );
        let first = service.refresh_startup_probe();
        assert_eq!(first.startup_probe.as_ref().unwrap().generation, 1);
        agent_store.with(|s| s.fail = Some("closed".into()));
        let second = service.refresh_startup_probe();
        let probe = second.startup_probe.expect("previous view retained");
        // A failed attempt never erases observed evidence nor mislabels a
        // closed store as Absent.
        assert!(probe.diagnostic.is_some());
        assert_eq!(probe.root_counts.present, 1);
    }

    // -- Activation Health ---------------------------------------------------

    #[test]
    fn first_health_view_loads_persisted_observations_as_stale() {
        let activation_store = StubActivationStore::new().with_rows(vec![stored_row(
            "skill-a",
            "t1",
            "/Users/me/agents/skills/skill-a",
            "/Users/me/agents/skills/skill-a",
            Some(ActivationObservedState::Present),
            Some(1_700_000_000_000),
        )]);
        let (service, _agent_fs, _fs, _activation) = harness(
            StubAgentFs::new(vec![]),
            StubFs::new(),
            StubAgentStore::new(1),
            activation_store,
        );
        let snapshot = service.snapshot();
        let health = snapshot.activation_health.expect("persisted view");
        assert_eq!(health.generation, 0);
        assert_eq!(health.status, ObservationStatus::Stale);
        assert_eq!(health.target_group_counts.target_groups, 1);
        assert_eq!(health.target_group_counts.entries_total, 1);
        assert_eq!(health.target_group_counts.entries_unknown, 1);
        let page = service
            .observation_page(
                ObservationKind::ActivationHealth,
                0,
                ObservationCursor { offset: 0 },
                64,
            )
            .expect("stale page readable");
        let row = match &page.rows[0] {
            ObservationRow::ActivationHealth(row) => row,
            _ => panic!("expected health row"),
        };
        assert!(row.stale);
        assert_eq!(row.observed_state, None);
        assert_eq!(row.previous_state, Some(ActivationObservedState::Present));
    }

    #[test]
    fn health_run_persists_per_target_after_cas() {
        let agent_store = StubAgentStore::new(2);
        agent_store
            .with(|s| s.roots = vec![root("t1", "/Users/me/t1", AgentRootRole::ActivationTarget)]);
        agent_store.with(|s| {
            s.configurations = vec![StoredAgentConfiguration {
                agent_id: "agent-1".into(),
                origin: AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: "Mine".into(),
                name_identity_key: "mine".into(),
                compatibility: crate::core::domain::Compatibility::Unknown,
                project_skills_dir: None,
                created_at: "t".into(),
                updated_at: "t".into(),
                memberships: vec![StoredAgentRootMembership {
                    root_id: "t1".into(),
                    role: AgentRootRole::ActivationTarget,
                }],
            }]
        });
        let fs = StubFs::new().entries(vec![
            (
                "/Users/me/t1/skill-a",
                ActivationEntrySnapshot::Symlink {
                    target: PathBuf::from("/Users/me/t1/skill-a"),
                },
            ),
            (
                "/Users/me/t1/skill-b",
                ActivationEntrySnapshot::Symlink {
                    target: PathBuf::from("/Users/me/t1/skill-b"),
                },
            ),
        ]);
        let fs = {
            let mut stub = fs;
            stub.readable
                .insert(PathBuf::from("/Users/me/t1/skill-a"), true);
            stub.readable
                .insert(PathBuf::from("/Users/me/t1/skill-b"), false);
            stub
        };
        let activation_store = StubActivationStore::new().with_rows(vec![
            stored_row(
                "skill-a",
                "t1",
                "/Users/me/t1/skill-a",
                "/Users/me/t1/skill-a",
                None,
                None,
            ),
            stored_row(
                "skill-b",
                "t1",
                "/Users/me/t1/skill-b",
                "/Users/me/t1/skill-b",
                Some(ActivationObservedState::Occupied),
                Some(1),
            ),
        ]);
        let (service, _agent_fs, _fs, activation_store) = harness(
            StubAgentFs::new(vec![(
                "/Users/me/t1",
                AgentRootProbe::Present {
                    canonical_path: PathBuf::from("/Users/me/t1"),
                },
            )]),
            fs,
            agent_store,
            activation_store,
        );
        service.refresh_activation_health(None);
        let snapshot = wait_health(&service, 5_000);
        let health = snapshot.activation_health.expect("health present");
        assert_eq!(health.status, ObservationStatus::Observed);
        assert_eq!(health.generation, 1);
        assert_eq!(health.target_group_counts.target_groups, 1);
        assert_eq!(health.target_group_counts.observed_groups, 1);
        assert_eq!(health.target_group_counts.entries_present, 1);
        assert_eq!(health.target_group_counts.entries_unhealthy, 1);
        let persisted = activation_store.recorded.lock().unwrap().clone();
        assert_eq!(persisted.len(), 1, "one atomic write per Target group");
        assert_eq!(persisted[0].len(), 2);
        assert_eq!(persisted[0][0].skill_id.0, "skill-a");
        assert_eq!(
            persisted[0][0].observed_state,
            ActivationObservedState::Present
        );
        assert_eq!(
            persisted[0][1].observed_state,
            ActivationObservedState::Dangling
        );
    }

    #[test]
    fn health_target_failure_keeps_old_observation_and_other_target_continues() {
        let agent_store = StubAgentStore::new(2);
        agent_store.with(|s| {
            s.roots = vec![
                root("t1", "/Users/me/t1", AgentRootRole::ActivationTarget),
                root("t2", "/Users/me/t2", AgentRootRole::ActivationTarget),
            ]
        });
        agent_store.with(|s| {
            s.configurations = vec![StoredAgentConfiguration {
                agent_id: "agent-1".into(),
                origin: AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: "Mine".into(),
                name_identity_key: "mine".into(),
                compatibility: crate::core::domain::Compatibility::Unknown,
                project_skills_dir: None,
                created_at: "t".into(),
                updated_at: "t".into(),
                memberships: vec![
                    StoredAgentRootMembership {
                        root_id: "t1".into(),
                        role: AgentRootRole::ActivationTarget,
                    },
                    StoredAgentRootMembership {
                        root_id: "t2".into(),
                        role: AgentRootRole::ActivationTarget,
                    },
                ],
            }]
        });
        let mut fs = StubFs::new();
        fs.entry_errors
            .insert(PathBuf::from("/Users/me/t1/skill-a"), "injected".into());
        fs.entry_outcomes.insert(
            PathBuf::from("/Users/me/t2/skill-b"),
            ActivationEntrySnapshot::Symlink {
                target: PathBuf::from("/Users/me/t2/skill-b"),
            },
        );
        fs.readable
            .insert(PathBuf::from("/Users/me/t2/skill-b"), true);
        let activation_store = StubActivationStore::new().with_rows(vec![
            stored_row(
                "skill-a",
                "t1",
                "/Users/me/t1/skill-a",
                "/Users/me/t1/skill-a",
                Some(ActivationObservedState::Present),
                Some(9),
            ),
            stored_row(
                "skill-b",
                "t2",
                "/Users/me/t2/skill-b",
                "/Users/me/t2/skill-b",
                Some(ActivationObservedState::Present),
                Some(9),
            ),
        ]);
        let (service, _agent_fs, _fs, activation_store) = harness(
            StubAgentFs::new(vec![
                (
                    "/Users/me/t1",
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/t1"),
                    },
                ),
                (
                    "/Users/me/t2",
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/t2"),
                    },
                ),
            ]),
            fs,
            agent_store,
            activation_store,
        );
        service.refresh_activation_health(None);
        let snapshot = wait_health(&service, 5_000);
        let health = snapshot.activation_health.unwrap();
        assert_eq!(health.status, ObservationStatus::Observed);
        let page = service
            .observation_page(
                ObservationKind::ActivationHealth,
                health.generation,
                ObservationCursor { offset: 0 },
                64,
            )
            .unwrap();
        let mut t1 = None;
        let mut t2 = None;
        for row in &page.rows {
            match row {
                ObservationRow::ActivationHealth(row) if row.target_root_id == "t1" => {
                    t1 = Some(row)
                }
                ObservationRow::ActivationHealth(row) if row.target_root_id == "t2" => {
                    t2 = Some(row)
                }
                _ => {}
            }
        }
        let t1 = t1.expect("t1 row");
        // t1's entry failed: old observation kept as Unknown/Stale.
        assert_eq!(t1.previous_state, Some(ActivationObservedState::Present));
        assert_eq!(t1.observed_state, None);
        assert!(t1.stale);
        assert!(t1.diagnostic.is_some());
        assert_eq!(
            t1.checked_at_ms,
            Some(9),
            "failed row keeps its old checked_at"
        );
        // t2 is fresh and observed.
        let t2 = t2.expect("t2 row");
        assert_eq!(t2.observed_state, Some(ActivationObservedState::Present));
        assert!(!t2.stale);
        assert_eq!(t2.checked_at_ms, Some(1_800_000_000_000));
        // Only t2's fresh rows were persisted; t1 was never written.
        let persisted = activation_store.recorded.lock().unwrap().clone();
        let persisted_rows = persisted.iter().flatten().collect::<Vec<_>>();
        assert!(
            persisted_rows.iter().all(|row| row.target_root_id == "t2"),
            "failed Target is never persisted: {:?}",
            persisted_rows
        );
    }

    #[test]
    fn health_cas_mismatch_persists_nothing() {
        let agent_store = StubAgentStore::new(2);
        agent_store
            .with(|s| s.roots = vec![root("t1", "/Users/me/t1", AgentRootRole::ActivationTarget)]);
        agent_store.with(|s| {
            s.configurations = vec![StoredAgentConfiguration {
                agent_id: "agent-1".into(),
                origin: AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: "Mine".into(),
                name_identity_key: "mine".into(),
                compatibility: crate::core::domain::Compatibility::Unknown,
                project_skills_dir: None,
                created_at: "t".into(),
                updated_at: "t".into(),
                memberships: vec![StoredAgentRootMembership {
                    root_id: "t1".into(),
                    role: AgentRootRole::ActivationTarget,
                }],
            }]
        });
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let mut fs = StubFs::new();
        fs.entry_outcomes.insert(
            PathBuf::from("/Users/me/t1/skill-a"),
            ActivationEntrySnapshot::Symlink {
                target: PathBuf::from("/Users/me/t1/skill-a"),
            },
        );
        fs.readable
            .insert(PathBuf::from("/Users/me/t1/skill-a"), true);
        fs.block
            .lock()
            .unwrap()
            .insert(PathBuf::from("/Users/me/t1/skill-a"), release_rx);
        fs.started.lock().unwrap().replace(started_tx);
        let activation_store = StubActivationStore::new().with_rows(vec![stored_row(
            "skill-a",
            "t1",
            "/Users/me/t1/skill-a",
            "/Users/me/t1/skill-a",
            Some(ActivationObservedState::Missing),
            Some(9),
        )]);
        let (service, _agent_fs, _fs, activation_store, gate) = harness_with_gate(
            bound_gate(),
            StubAgentFs::new(vec![(
                "/Users/me/t1",
                AgentRootProbe::Present {
                    canonical_path: PathBuf::from("/Users/me/t1"),
                },
            )]),
            fs,
            agent_store,
            activation_store,
        );
        let service = Arc::new(service);
        service.refresh_activation_health(None);
        // The t1 entry check blocks; change the WriteGate generation while
        // the group is in flight, then release: the CAS must fail closed.
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("entry check started");
        let home = BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            PathBuf::from("/tmp/skill-man-home"),
        );
        let _ = gate.transition_to(WriteGateState::Open(home));
        release_tx.send(()).unwrap();
        let snapshot = wait_health(&service, 5_000);
        let health = snapshot.activation_health.expect("health present");
        assert_eq!(health.status, ObservationStatus::Observed);
        assert_eq!(health.target_group_counts.failed_groups, 1);
        assert_eq!(
            activation_store.recorded.lock().unwrap().len(),
            0,
            "a mid-run generation change must never persist observations"
        );
        let page = service
            .observation_page(
                ObservationKind::ActivationHealth,
                health.generation,
                ObservationCursor { offset: 0 },
                64,
            )
            .unwrap();
        let row = match &page.rows[0] {
            ObservationRow::ActivationHealth(row) => row,
            _ => panic!("health row"),
        };
        assert!(row.stale);
        assert_eq!(row.observed_state, None);
        assert_eq!(row.previous_state, Some(ActivationObservedState::Missing));
        assert!(row.diagnostic.is_some());
    }

    #[test]
    fn health_read_only_gate_never_persists() {
        let agent_store = StubAgentStore::new(2);
        agent_store
            .with(|s| s.roots = vec![root("t1", "/Users/me/t1", AgentRootRole::ActivationTarget)]);
        agent_store.with(|s| {
            s.configurations = vec![StoredAgentConfiguration {
                agent_id: "agent-1".into(),
                origin: AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: "Mine".into(),
                name_identity_key: "mine".into(),
                compatibility: crate::core::domain::Compatibility::Unknown,
                project_skills_dir: None,
                created_at: "t".into(),
                updated_at: "t".into(),
                memberships: vec![StoredAgentRootMembership {
                    root_id: "t1".into(),
                    role: AgentRootRole::ActivationTarget,
                }],
            }]
        });
        let mut fs = StubFs::new();
        fs.entry_outcomes.insert(
            PathBuf::from("/Users/me/t1/skill-a"),
            ActivationEntrySnapshot::Symlink {
                target: PathBuf::from("/Users/me/t1/skill-a"),
            },
        );
        fs.readable
            .insert(PathBuf::from("/Users/me/t1/skill-a"), true);
        let activation_store = StubActivationStore::new().with_rows(vec![stored_row(
            "skill-a",
            "t1",
            "/Users/me/t1/skill-a",
            "/Users/me/t1/skill-a",
            Some(ActivationObservedState::Present),
            Some(9),
        )]);
        let gate = Arc::new(WriteGate::new(WriteGateState::CatalogReadOnly {
            reason: ReadOnlyReason::IntegrityFailed,
        }));
        let service = ObservationService::new(
            Arc::new(StubAgentFs::new(vec![(
                "/Users/me/t1",
                AgentRootProbe::Present {
                    canonical_path: PathBuf::from("/Users/me/t1"),
                },
            )])),
            Arc::new(fs),
            test_presets(),
            gate,
            Arc::new(agent_store),
            Arc::new(activation_store.clone()),
            Arc::new(StubClock),
        );
        service.refresh_activation_health(None);
        let snapshot = wait_health(&service, 5_000);
        let health = snapshot.activation_health.unwrap();
        assert_eq!(health.status, ObservationStatus::Observed);
        assert_eq!(health.target_group_counts.failed_groups, 1);
        assert_eq!(
            activation_store.recorded.lock().unwrap().len(),
            0,
            "CatalogReadOnly never persists health"
        );
        let page = service
            .observation_page(
                ObservationKind::ActivationHealth,
                health.generation,
                ObservationCursor { offset: 0 },
                64,
            )
            .unwrap();
        let row = match &page.rows[0] {
            ObservationRow::ActivationHealth(row) => row,
            _ => panic!("health row"),
        };
        assert!(row.stale);
        assert_eq!(row.previous_state, Some(ActivationObservedState::Present));
        assert_eq!(row.observed_state, None);
    }

    #[test]
    fn target_scoped_refresh_only_rechecks_affected_targets() {
        let agent_store = StubAgentStore::new(2);
        agent_store.with(|s| {
            s.roots = vec![
                root("t1", "/Users/me/t1", AgentRootRole::ActivationTarget),
                root("t2", "/Users/me/t2", AgentRootRole::ActivationTarget),
            ]
        });
        agent_store.with(|s| {
            s.configurations = vec![StoredAgentConfiguration {
                agent_id: "agent-1".into(),
                origin: AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: "Mine".into(),
                name_identity_key: "mine".into(),
                compatibility: crate::core::domain::Compatibility::Unknown,
                project_skills_dir: None,
                created_at: "t".into(),
                updated_at: "t".into(),
                memberships: vec![
                    StoredAgentRootMembership {
                        root_id: "t1".into(),
                        role: AgentRootRole::ActivationTarget,
                    },
                    StoredAgentRootMembership {
                        root_id: "t2".into(),
                        role: AgentRootRole::ActivationTarget,
                    },
                ],
            }]
        });
        let mut fs = StubFs::new();
        fs.entry_outcomes.insert(
            PathBuf::from("/Users/me/t1/skill-a"),
            ActivationEntrySnapshot::Symlink {
                target: PathBuf::from("/Users/me/t1/skill-a"),
            },
        );
        fs.entry_outcomes.insert(
            PathBuf::from("/Users/me/t2/skill-b"),
            ActivationEntrySnapshot::Symlink {
                target: PathBuf::from("/Users/me/t2/skill-b"),
            },
        );
        fs.readable
            .insert(PathBuf::from("/Users/me/t1/skill-a"), true);
        fs.readable
            .insert(PathBuf::from("/Users/me/t2/skill-b"), true);
        let activation_store = StubActivationStore::new().with_rows(vec![
            stored_row(
                "skill-a",
                "t1",
                "/Users/me/t1/skill-a",
                "/Users/me/t1/skill-a",
                Some(ActivationObservedState::Present),
                Some(9),
            ),
            stored_row(
                "skill-b",
                "t2",
                "/Users/me/t2/skill-b",
                "/Users/me/t2/skill-b",
                Some(ActivationObservedState::Present),
                Some(9),
            ),
        ]);
        let (service, _agent_fs, fs, _activation) = harness(
            StubAgentFs::new(vec![
                (
                    "/Users/me/t1",
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/t1"),
                    },
                ),
                (
                    "/Users/me/t2",
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/t2"),
                    },
                ),
            ]),
            fs,
            agent_store,
            activation_store,
        );
        // Full run first (both targets observed).
        service.refresh_activation_health(None);
        wait_health(&service, 5_000);
        let calls_after_full = fs.entry_calls.load(AtomicOrdering::SeqCst);
        // Target-scoped refresh: only t1!
        service.refresh_activation_health(Some(&["t1".to_string()]));
        wait_health(&service, 5_000);
        let calls_after_scoped = fs.entry_calls.load(AtomicOrdering::SeqCst);
        assert_eq!(
            calls_after_scoped - calls_after_full,
            1,
            "only the affected Target's entries are re-checked"
        );
    }

    #[test]
    fn unresponsive_target_is_isolated_and_other_target_continues() {
        let agent_store = StubAgentStore::new(2);
        agent_store.with(|s| {
            s.roots = vec![
                root("t1", "/Users/me/t1", AgentRootRole::ActivationTarget),
                root("t2", "/Users/me/t2", AgentRootRole::ActivationTarget),
            ]
        });
        agent_store.with(|s| {
            s.configurations = vec![StoredAgentConfiguration {
                agent_id: "agent-1".into(),
                origin: AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: "Mine".into(),
                name_identity_key: "mine".into(),
                compatibility: crate::core::domain::Compatibility::Unknown,
                project_skills_dir: None,
                created_at: "t".into(),
                updated_at: "t".into(),
                memberships: vec![
                    StoredAgentRootMembership {
                        root_id: "t1".into(),
                        role: AgentRootRole::ActivationTarget,
                    },
                    StoredAgentRootMembership {
                        root_id: "t2".into(),
                        role: AgentRootRole::ActivationTarget,
                    },
                ],
            }]
        });
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let mut fs = StubFs::new();
        fs.entry_outcomes.insert(
            PathBuf::from("/Users/me/t2/skill-b"),
            ActivationEntrySnapshot::Symlink {
                target: PathBuf::from("/Users/me/t2/skill-b"),
            },
        );
        fs.readable
            .insert(PathBuf::from("/Users/me/t2/skill-b"), true);
        fs.block
            .lock()
            .unwrap()
            .insert(PathBuf::from("/Users/me/t1/skill-a"), release_rx);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        fs.started.lock().unwrap().replace(started_tx);
        let activation_store = StubActivationStore::new().with_rows(vec![
            stored_row(
                "skill-a",
                "t1",
                "/Users/me/t1/skill-a",
                "/Users/me/t1/skill-a",
                Some(ActivationObservedState::Present),
                Some(9),
            ),
            stored_row(
                "skill-b",
                "t2",
                "/Users/me/t2/skill-b",
                "/Users/me/t2/skill-b",
                Some(ActivationObservedState::Present),
                Some(9),
            ),
        ]);
        let (service, _agent_fs, _fs, activation_store) = harness(
            StubAgentFs::new(vec![
                (
                    "/Users/me/t1",
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/t1"),
                    },
                ),
                (
                    "/Users/me/t2",
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/t2"),
                    },
                ),
            ]),
            fs,
            agent_store,
            activation_store,
        );
        // The window comfortably exceeds worker spawn latency on a loaded
        // harness; the claim resets it so only a truly blocked group is
        // typed Unresponsive.
        let service = service.with_unresponsive_ms(1500);
        service.refresh_activation_health(None);
        // Wait until t1's entry check blocks (t2 proceeds immediately).
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("t1 blocked");
        // The watchdog isolates t1 while t2 completes.
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let snapshot = service.snapshot();
            if let Some(health) = &snapshot.activation_health {
                if health.target_group_counts.unresponsive_groups >= 1 {
                    break;
                }
            }
            if Instant::now() >= deadline {
                panic!("t1 was not isolated");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        release_tx.send(()).unwrap();
        let snapshot = wait_health(&service, 5_000);
        let health = snapshot.activation_health.unwrap();
        assert_eq!(health.target_group_counts.unresponsive_groups, 1);
        assert_eq!(health.target_group_counts.failed_groups, 0);
        let page = service
            .observation_page(
                ObservationKind::ActivationHealth,
                health.generation,
                ObservationCursor { offset: 0 },
                64,
            )
            .unwrap();
        let mut t1 = None;
        let mut t2 = None;
        for row in &page.rows {
            match row {
                ObservationRow::ActivationHealth(row) if row.target_root_id == "t1" => {
                    t1 = Some(row)
                }
                ObservationRow::ActivationHealth(row) if row.target_root_id == "t2" => {
                    t2 = Some(row)
                }
                _ => {}
            }
        }
        let t1 = t1.unwrap();
        assert!(t1.stale);
        assert_eq!(t1.observed_state, None);
        assert_eq!(t1.previous_state, Some(ActivationObservedState::Present));
        assert!(t1.diagnostic.is_some());
        let t2 = t2.unwrap();
        assert!(!t2.stale);
        assert_eq!(t2.observed_state, Some(ActivationObservedState::Present));
        // Only t2 was persisted (t1 isolated).
        let persisted = activation_store.recorded.lock().unwrap().clone();
        let persisted_rows = persisted.iter().flatten().collect::<Vec<_>>();
        assert_eq!(persisted_rows.len(), 1);
        assert_eq!(persisted_rows[0].target_root_id, "t2");
    }

    #[test]
    fn health_empty_scope_completes_observed() {
        let (service, _agent_fs, _fs, _activation) = harness(
            StubAgentFs::new(vec![]),
            StubFs::new(),
            StubAgentStore::new(1),
            StubActivationStore::new(),
        );
        service.refresh_activation_health(None);
        let snapshot = wait_health(&service, 5_000);
        let health = snapshot.activation_health.unwrap();
        assert_eq!(health.status, ObservationStatus::Observed);
        assert_eq!(health.target_group_counts.target_groups, 0);
    }

    #[test]
    fn health_scoped_unknown_target_completes() {
        let (service, _agent_fs, _fs, _activation) = harness(
            StubAgentFs::new(vec![]),
            StubFs::new(),
            StubAgentStore::new(1),
            StubActivationStore::new(),
        );
        service.refresh_activation_health(Some(&["missing".to_string()]));
        let snapshot = wait_health(&service, 5_000);
        let health = snapshot.activation_health.unwrap();
        assert_eq!(health.status, ObservationStatus::Observed);
    }

    // -- observation_page ----------------------------------------------------

    #[test]
    fn observation_page_typed_stale_and_not_found() {
        let (service, _agent_fs, _fs, _activation) = harness(
            StubAgentFs::new(vec![]),
            StubFs::new(),
            StubAgentStore::new(1),
            StubActivationStore::new(),
        );
        // Never initialized: not-found.
        assert_eq!(
            service.observation_page(
                ObservationKind::StartupProbe,
                0,
                ObservationCursor { offset: 0 },
                16,
            ),
            Err(ObservationPageError::NotFound)
        );
        // A health view exists at generation 0 after the lazy load; page 1
        // must be typed stale, never a fallback.
        service.snapshot();
        // closed activation store: still initializes on first snapshot...
        assert!(matches!(
            service.observation_page(
                ObservationKind::ActivationHealth,
                1,
                ObservationCursor { offset: 0 },
                16,
            ),
            Err(ObservationPageError::Stale {
                current_generation: 0
            })
        ));
        assert!(matches!(
            service.observation_page(
                ObservationKind::ActivationHealth,
                0,
                ObservationCursor { offset: 0 },
                16,
            ),
            Ok(page) if page.rows.is_empty()
        ));
    }

    #[test]
    fn observation_page_paginates_stable_order() {
        let agent_store = StubAgentStore::new(3);
        agent_store.with(|s| {
            s.roots = (0..3)
                .map(|index| {
                    root(
                        &format!("r{index}"),
                        &format!("/Users/me/skills{index}"),
                        AgentRootRole::ScanOnly,
                    )
                })
                .collect()
        });
        let (service, _agent_fs, _fs, _activation) = harness(
            StubAgentFs::new(
                (0..3)
                    .map(|index| {
                        let path: &'static str =
                            Box::leak(format!("/Users/me/skills{index}").into_boxed_str());
                        (
                            path,
                            AgentRootProbe::Present {
                                canonical_path: PathBuf::from(format!("/Users/me/skills{index}")),
                            },
                        )
                    })
                    .collect(),
            ),
            StubFs::new(),
            agent_store,
            StubActivationStore::new(),
        );
        service.refresh_startup_probe();
        let first = service
            .observation_page(
                ObservationKind::StartupProbe,
                1,
                ObservationCursor { offset: 0 },
                2,
            )
            .expect("first page");
        assert_eq!(first.rows.len(), 2);
        assert_eq!(first.next_offset, Some(2));
        let second = service
            .observation_page(
                ObservationKind::StartupProbe,
                1,
                ObservationCursor { offset: 2 },
                2,
            )
            .expect("second page");
        assert_eq!(second.rows.len(), 1);
        assert_eq!(second.next_offset, None);
    }

    #[test]
    fn project_skills_dir_never_enters_probe_or_health() {
        let agent_store = StubAgentStore::new(3);
        agent_store
            .with(|s| s.roots = vec![root("r1", "/Users/me/skills", AgentRootRole::ScanOnly)]);
        agent_store.with(|s| {
            s.configurations = vec![StoredAgentConfiguration {
                agent_id: "agent-1".into(),
                origin: AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: "Mine".into(),
                name_identity_key: "mine".into(),
                compatibility: crate::core::domain::Compatibility::Unknown,
                project_skills_dir: Some(PathBuf::from(".agents/skills")),
                created_at: "t".into(),
                updated_at: "t".into(),
                memberships: vec![],
            }]
        });
        let (service, agent_fs, fs, _activation) = harness(
            StubAgentFs::new(vec![]),
            StubFs::new(),
            agent_store,
            StubActivationStore::new(),
        );
        service.refresh_startup_probe();
        assert_eq!(agent_fs.calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(fs.entry_calls.load(AtomicOrdering::SeqCst), 0);
        let probe = service.snapshot().startup_probe.unwrap();
        assert_eq!(probe.root_counts.total, 1);
    }
}
