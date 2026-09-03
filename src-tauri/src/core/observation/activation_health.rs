//! Activation Health Observation (spec §4.10; ADR-0020): Target-scoped,
//! read-mostly observation of the managed entry symlink and its final
//! entity, scheduled on the same observation module as Agent Detection and
//! Startup Probe.
//!
//! Contracts enforced here:
//! - the persisted (Catalog) row set is loaded at generation start so the
//!   first screen shows old observations as `Stale/Checking`;
//! - every Target group is observed independently; a group failure (CAS
//!   mismatch, persist error, zero-progress Unresponsive) keeps the old
//!   observation as Unknown/Stale + diagnostic and never writes `Missing`/
//!   `Broken` — other Targets continue;
//! - observation persists only after the CAS: current `home_id`, WriteGate
//!   generation, Agent Configuration generation and Target identity still
//!   match the frozen start facts; a non-`Open` gate never persists;
//! - health never advances the filesystem-mutation generation nor makes
//!   the Scan Report stale (durable writes keep `snapshot_version` stable);
//! - a Target with zero entry progress for `unresponsive_ms` is typed
//!   Unresponsive and isolated; the whole run over `health_slow_ms` only
//!   marks `Slow` (never truncates or downgrades);
//! - startup, successful Enable/Disable/Repair, Target configuration change
//!   and explicit Retry schedule only the affected Targets; the project
//!   one-shot softlink never enters.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::core::domain::{ActivationObservedState, SkillId};
use crate::core::observation::types::{
    ActivationHealthCounts, ActivationHealthRow, ObservationStatus,
};
use crate::core::write_gate::{WriteGate, WriteGateState};
use crate::seams::activation_health::ActivationEntryFileSystem;
use crate::seams::activation_store::{
    ActivationObservation, ActivationStore, StoredActivationObservation,
};
use crate::seams::agent_configuration_fs::{AgentConfigurationFileSystem, AgentRootProbe};
use crate::seams::agent_configuration_store::AgentConfigurationStore;
use crate::seams::clock::Clock;
use crate::seams::filesystem::ActivationEntrySnapshot;

pub(crate) const DEFAULT_UNRESPONSIVE_MS: u64 = 30_000;
pub(crate) const DEFAULT_HEALTH_SLOW_MS: u64 = 5_000;
const HEALTH_TARGET_WORKERS: usize = 4;
const HEALTH_WATCHDOG_TICK_MS: u64 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GroupStatus {
    /// Not scheduled by the current run (carried view).
    Pending,
    Checking,
    Observed,
    CasFailed,
    Unresponsive,
}

#[derive(Clone, Debug)]
pub(crate) struct FrozenTarget {
    pub(crate) path_identity_key: String,
    pub(crate) canonical_path: Option<PathBuf>,
}

/// Facts frozen at run start; the CAS at persist compares the live state
/// against these (spec §4.10 / ADR-0020).
#[derive(Clone, Debug)]
pub(crate) struct FrozenHealthFacts {
    pub(crate) home_id: Option<String>,
    pub(crate) write_gate_generation: u64,
    pub(crate) agent_configuration_generation: u64,
    /// Target identity at generation start (`target_root_id` → facts).
    pub(crate) targets: BTreeMap<String, FrozenTarget>,
}

#[derive(Clone, Debug)]
pub(crate) struct HealthGroup {
    pub(crate) target_root_id: String,
    pub(crate) status: GroupStatus,
    pub(crate) rows: Vec<ActivationHealthRow>,
    pub(crate) entries_total: u64,
    pub(crate) checked: u64,
    /// Last entry/probe progress (watchdog window).
    pub(crate) last_progress: Instant,
    pub(crate) diagnostic: Option<String>,
}

impl HealthGroup {
    pub(crate) fn new(target_root_id: String, rows: Vec<ActivationHealthRow>) -> Self {
        Self {
            entries_total: rows.len() as u64,
            target_root_id,
            status: GroupStatus::Pending,
            rows,
            checked: 0,
            last_progress: Instant::now(),
            diagnostic: None,
        }
    }
}

/// The cross-startup view: one `Pending` group per Target built from the
/// persisted rows, all marked stale — the first screen shows the old
/// observations as `Stale/Checking` until a run re-observes them (ADR-0020).
pub(crate) fn initial_groups(stored: &[StoredActivationObservation]) -> Vec<HealthGroup> {
    let mut groups: BTreeMap<String, Vec<StoredActivationObservation>> = BTreeMap::new();
    for observation in stored {
        groups
            .entry(observation.target_root_id.clone())
            .or_default()
            .push(observation.clone());
    }
    let mut groups = groups
        .into_iter()
        .map(|(target, rows)| {
            let mut rows = rows;
            rows.sort_by(|left, right| left.skill_id.0.cmp(&right.skill_id.0));
            HealthGroup::new(
                target,
                rows.iter().map(|row| row_from_stored(row, true)).collect(),
            )
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| left.target_root_id.cmp(&right.target_root_id));
    groups
}

/// The current generation view: bounded counts plus the immutable row set
/// (rows never reorder; only per-row state and per-group status mutate).
#[derive(Clone, Debug)]
pub(crate) struct HealthSharedState {
    pub(crate) generation: u64,
    pub(crate) status: ObservationStatus,
    pub(crate) slow: bool,
    pub(crate) diagnostic: Option<String>,
    pub(crate) groups: Vec<HealthGroup>,
    pub(crate) initialized: bool,
}

impl HealthSharedState {
    pub(crate) fn fresh() -> Self {
        Self {
            generation: 0,
            status: ObservationStatus::Unknown,
            slow: false,
            diagnostic: None,
            groups: Vec::new(),
            initialized: false,
        }
    }

    pub(crate) fn counts(&self) -> ActivationHealthCounts {
        let mut counts = ActivationHealthCounts::default();
        for group in &self.groups {
            counts.target_groups += 1;
            match group.status {
                GroupStatus::Observed => counts.observed_groups += 1,
                GroupStatus::CasFailed => counts.failed_groups += 1,
                GroupStatus::Unresponsive => counts.unresponsive_groups += 1,
                GroupStatus::Pending | GroupStatus::Checking => {}
            }
            for row in &group.rows {
                counts.entries_total += 1;
                match row.observed_state {
                    Some(ActivationObservedState::Present) => counts.entries_present += 1,
                    Some(_) => counts.entries_unhealthy += 1,
                    None => counts.entries_unknown += 1,
                }
            }
        }
        counts
    }

    /// The flat row sequence in generation-stable order
    /// (target_root_id, then skill_id).
    pub(crate) fn flat_rows(&self) -> Vec<ActivationHealthRow> {
        self.groups
            .iter()
            .flat_map(|group| group.rows.iter().cloned())
            .collect()
    }
}

/// One run's coordination handles; `request_stop` is the supersede path.
pub(crate) struct HealthRunHandle {
    pub(crate) stop: Arc<AtomicBool>,
}

impl HealthRunHandle {
    pub(crate) fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Immutable per-run dependencies: the worker threads only touch these plus
/// the shared state; the service itself is never captured (no Arc cycle).
pub(crate) struct HealthRunContext {
    pub(crate) activation_store: Arc<dyn ActivationStore>,
    pub(crate) agent_store: Arc<dyn AgentConfigurationStore>,
    pub(crate) config_fs: Arc<dyn AgentConfigurationFileSystem>,
    pub(crate) filesystem: Arc<dyn ActivationEntryFileSystem>,
    pub(crate) write_gate: Arc<WriteGate>,
    pub(crate) clock: Arc<dyn Clock>,
}

/// Start (or supersede any active run with) a new health generation.
/// `in_scope` empty = all Targets; otherwise only the affected Targets are
/// re-observed and everything else is carried from the previous view.
#[allow(clippy::too_many_arguments)]
pub(crate) fn trigger(
    context: &HealthRunContext,
    shared: Arc<Mutex<HealthSharedState>>,
    in_scope: Vec<String>,
    emit: Arc<dyn Fn(bool) + Send + Sync>,
    unresponsive_ms: u64,
    health_slow_ms: u64,
) -> HealthRunHandle {
    // Advance the generation synchronously so a still-winding-down old run
    // never overwrites the new run's final state.
    let generation = {
        let mut state = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.status = ObservationStatus::Checking;
        state.slow = false;
        state.diagnostic = None;
        state.generation
    };
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();

    let thread_shared = shared.clone();
    let thread_emit = emit.clone();
    let context_owned = HealthRunContext {
        activation_store: context.activation_store.clone(),
        agent_store: context.agent_store.clone(),
        config_fs: context.config_fs.clone(),
        filesystem: context.filesystem.clone(),
        write_gate: context.write_gate.clone(),
        clock: context.clock.clone(),
    };
    let thread = std::thread::Builder::new()
        .name("health-run".into())
        .spawn(move || {
            health_run(
                &context_owned,
                thread_shared,
                generation,
                in_scope,
                thread_stop,
                thread_emit,
                unresponsive_ms,
                health_slow_ms,
            );
        })
        .expect("spawn Activation health run thread");
    emit(true);
    let _ = thread;
    HealthRunHandle { stop }
}

#[allow(clippy::too_many_arguments)]
fn health_run(
    context: &HealthRunContext,
    shared: Arc<Mutex<HealthSharedState>>,
    generation: u64,
    in_scope: Vec<String>,
    stop: Arc<AtomicBool>,
    emit: Arc<dyn Fn(bool) + Send + Sync>,
    unresponsive_ms: u64,
    health_slow_ms: u64,
) {
    let started = Instant::now();
    let all_targets = in_scope.is_empty();
    // Load the durable row set and freeze the start facts first: any
    // failure keeps the previous view and marks it Stale (fail-closed,
    // never a guessed state).
    let (rows, frozen) = match load_facts(context) {
        Ok(loaded) => loaded,
        Err(error) => {
            let mut state = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.generation != generation {
                return;
            }
            if state.initialized {
                state.status = ObservationStatus::Stale;
                state.diagnostic = Some(error);
                emit(true);
            }
            return;
        }
    };
    let frozen = Arc::new(frozen);
    let scope: BTreeSet<String> = if all_targets {
        rows.keys().cloned().collect()
    } else {
        in_scope.iter().cloned().collect()
    };
    {
        let mut state = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.generation != generation {
            return;
        }
        let previous_groups = std::mem::take(&mut state.groups);
        let mut groups = Vec::with_capacity(rows.len());
        for (target, target_rows) in &rows {
            let group = if scope.contains(target) {
                HealthGroup::new(
                    target.clone(),
                    target_rows
                        .iter()
                        .map(|row| row_from_stored(row, true))
                        .collect(),
                )
            } else {
                previous_groups
                    .iter()
                    .find(|previous| previous.target_root_id == *target)
                    .cloned()
                    .unwrap_or_else(|| {
                        HealthGroup::new(
                            target.clone(),
                            target_rows
                                .iter()
                                .map(|row| row_from_stored(row, true))
                                .collect(),
                        )
                    })
            };
            groups.push(group);
        }
        groups.sort_by(|left, right| left.target_root_id.cmp(&right.target_root_id));
        state.groups = groups;
        state.status = ObservationStatus::Checking;
        state.initialized = true;
    }
    emit(true);

    let queue: VecDeque<String> = if all_targets {
        rows.keys().cloned().collect()
    } else {
        in_scope
            .iter()
            .filter(|target| rows.contains_key(*target))
            .cloned()
            .collect()
    };
    // Fully out-of-scope run or empty scope: nothing to observe.
    if queue.is_empty() {
        finalize(&shared, generation, started, health_slow_ms, &emit);
        return;
    }

    let queue = Arc::new(Mutex::new(queue));
    let workers = queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .len()
        .min(HEALTH_TARGET_WORKERS);
    let mut joins = Vec::with_capacity(workers);
    for _ in 0..workers {
        let context = HealthRunContext {
            activation_store: context.activation_store.clone(),
            agent_store: context.agent_store.clone(),
            config_fs: context.config_fs.clone(),
            filesystem: context.filesystem.clone(),
            write_gate: context.write_gate.clone(),
            clock: context.clock.clone(),
        };
        let shared = shared.clone();
        let queue = queue.clone();
        let stop = stop.clone();
        let emit = emit.clone();
        let frozen = frozen.clone();
        joins.push(
            std::thread::Builder::new()
                .name("health-target".into())
                .spawn(move || {
                    loop {
                        let target = queue
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .pop_front();
                        let Some(target) = target else { break };
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        check_target(
                            &context, &shared, &target, generation, &stop, &emit, &frozen,
                        );
                    }
                })
                .expect("spawn health target worker"),
        );
    }

    let watchdog = {
        let shared_watch = shared.clone();
        let stop_watch = stop.clone();
        let emit_watch = emit.clone();
        std::thread::Builder::new()
            .name("health-watchdog".into())
            .spawn(move || {
                loop {
                    if stop_watch.load(Ordering::Acquire) {
                        break;
                    }
                    let mut isolated = false;
                    let any_active;
                    {
                        let mut state = shared_watch
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if state.generation != generation {
                            break;
                        }
                        any_active = state.groups.iter().any(|group| {
                            group.status == GroupStatus::Checking
                                || group.status == GroupStatus::Pending
                        });
                        for group in state
                            .groups
                            .iter_mut()
                            .filter(|group| group.status == GroupStatus::Checking)
                        {
                            let elapsed = group.last_progress.elapsed().as_millis() as u64;
                            if elapsed >= unresponsive_ms {
                                group.status = GroupStatus::Unresponsive;
                                group.diagnostic =
                                    Some(format!("unresponsive: {elapsed} ms without progress"));
                                for row in &mut group.rows {
                                    row.observed_state = None;
                                    row.stale = true;
                                    row.diagnostic = Some("target unresponsive".into());
                                }
                                isolated = true;
                            }
                        }
                    }
                    if isolated {
                        emit_watch(true);
                    }
                    if !any_active {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(HEALTH_WATCHDOG_TICK_MS));
                }
            })
            .expect("spawn health watchdog")
    };

    for join in joins {
        let _ = join.join();
    }
    let _ = watchdog.join();
    finalize(&shared, generation, started, health_slow_ms, &emit);
}

fn finalize(
    shared: &Arc<Mutex<HealthSharedState>>,
    generation: u64,
    started: Instant,
    health_slow_ms: u64,
    emit: &Arc<dyn Fn(bool) + Send + Sync>,
) {
    let mut state = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.generation != generation {
        return;
    }
    state.status = ObservationStatus::Observed;
    state.slow = started.elapsed().as_millis() as u64 >= health_slow_ms;
    state.diagnostic = None;
    drop(state);
    emit(true);
}

fn row_from_stored(stored: &StoredActivationObservation, stale: bool) -> ActivationHealthRow {
    ActivationHealthRow {
        skill_id: stored.skill_id.0.clone(),
        target_root_id: stored.target_root_id.clone(),
        entry_path: stored.expected_entry_path.clone(),
        expected_target_path: stored.expected_target_path.clone(),
        observed_state: None,
        previous_state: stored.observed_state,
        stale,
        diagnostic: None,
        checked_at_ms: stored.last_checked_at_ms,
    }
}

/// Load the durable row set grouped by Target and freeze the start facts.
/// The row set always comes from the current Catalog state.
fn load_facts(
    context: &HealthRunContext,
) -> Result<
    (
        BTreeMap<String, Vec<StoredActivationObservation>>,
        FrozenHealthFacts,
    ),
    String,
> {
    let gate = context.write_gate.snapshot();
    let (home_id, write_gate_generation) = match &gate.state {
        WriteGateState::Open(home) => (Some(home.home_id.0.clone()), gate.generation),
        _ => (None, gate.generation),
    };
    let snapshot = context
        .agent_store
        .agent_configuration_snapshot()
        .map_err(|error| format!("agent configuration unavailable: {error}"))?;
    let stored = context
        .activation_store
        .activation_observations()
        .map_err(|error| format!("activation observations unavailable: {error}"))?;
    let target_root_ids = snapshot
        .configurations
        .iter()
        .flat_map(|configuration| {
            configuration
                .memberships
                .iter()
                .filter(|membership| {
                    membership.role
                        == crate::core::agent_configuration::AgentRootRole::ActivationTarget
                })
                .map(|membership| membership.root_id.clone())
        })
        .collect::<BTreeSet<_>>();
    let mut targets = BTreeMap::new();
    for root in &snapshot.roots {
        if !target_root_ids.contains(&root.root_id) {
            continue;
        }
        let canonical_path = match context.config_fs.probe_root(&root.configured_path) {
            Ok(AgentRootProbe::Present { canonical_path }) => Some(canonical_path),
            _ => None,
        };
        targets.insert(
            root.root_id.clone(),
            FrozenTarget {
                path_identity_key: root.path_identity_key.clone(),
                canonical_path,
            },
        );
    }
    let mut rows = BTreeMap::<String, Vec<StoredActivationObservation>>::new();
    for observation in stored {
        rows.entry(observation.target_root_id.clone())
            .or_default()
            .push(observation);
    }
    for entry in rows.values_mut() {
        entry.sort_by(|left, right| left.skill_id.0.cmp(&right.skill_id.0));
    }
    Ok((
        rows,
        FrozenHealthFacts {
            home_id,
            write_gate_generation,
            agent_configuration_generation: snapshot.snapshot_version,
            targets,
        },
    ))
}

/// Observe one Target group: a per-entry fs failure keeps only that row's
/// old observation; only after every entry is checked the group is
/// CAS-guarded and persisted atomically.
fn check_target(
    context: &HealthRunContext,
    shared: &Arc<Mutex<HealthSharedState>>,
    target: &str,
    generation: u64,
    stop: &Arc<AtomicBool>,
    emit: &Arc<dyn Fn(bool) + Send + Sync>,
    frozen: &Arc<FrozenHealthFacts>,
) {
    let row_count: usize = 'claim: {
        let mut state = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.generation != generation {
            break 'claim 0;
        }
        let Some(group) = state
            .groups
            .iter_mut()
            .find(|group| group.target_root_id == target)
        else {
            break 'claim 0;
        };
        if group.status == GroupStatus::Checking {
            group.last_progress = Instant::now();
            break 'claim group.rows.len();
        }
        if group.status == GroupStatus::Pending {
            // The watchdog window starts at the actual work claim: a worker
            // that has not been scheduled yet is not "unresponsive".
            group.status = GroupStatus::Checking;
            group.last_progress = Instant::now();
            break 'claim group.rows.len();
        }
        break 'claim 0;
    };
    if row_count == 0 {
        return;
    }
    let mut observed = Vec::new();
    for index in 0..row_count {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let row_facts = {
            let state = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let group = state
                .groups
                .iter()
                .find(|group| group.target_root_id == target)
                .expect("group exists");
            if group.status != GroupStatus::Checking {
                return;
            }
            (
                group.rows[index].skill_id.clone(),
                group.rows[index].entry_path.clone(),
                group.rows[index].expected_target_path.clone(),
            )
        };
        let checked_at_ms = context.clock.unix_epoch_nanos() / 1_000_000;
        let outcome = classify_entry(context, &row_facts.1, &row_facts.2);
        let (observed_state, diagnostic) = match outcome {
            EntryOutcome::Observed(state) => (Some(state), None),
            EntryOutcome::Failed(diagnostic) => (None, Some(diagnostic)),
        };
        {
            let mut state = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let group = state
                .groups
                .iter_mut()
                .find(|group| group.target_root_id == target)
                .expect("group exists");
            if group.status != GroupStatus::Checking {
                break;
            }
            group.rows[index].observed_state = observed_state;
            group.rows[index].stale = observed_state.is_none();
            group.rows[index].diagnostic = diagnostic.clone();
            if observed_state.is_some() {
                group.rows[index].checked_at_ms = Some(u64::try_from(checked_at_ms).unwrap_or(0));
            }
            group.checked += 1;
            group.last_progress = Instant::now();
        }
        if let Some(state) = observed_state {
            observed.push(ActivationObservation {
                skill_id: SkillId(row_facts.0),
                target_root_id: target.to_owned(),
                observed_state: state,
            });
        }
        emit(false);
    }

    if stop.load(Ordering::Acquire) {
        return;
    }
    let Some(frozen_target) = frozen.targets.get(target).cloned() else {
        mark_failed(
            shared,
            target,
            "the Target is not in the Agent Configuration",
        );
        emit(true);
        return;
    };

    let cas = cas_and_persist(context, frozen, target, &frozen_target, &observed);
    match cas {
        Ok(()) => {
            let mut state = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let group = state
                .groups
                .iter_mut()
                .find(|group| group.target_root_id == target)
                .expect("group exists");
            if group.status == GroupStatus::Checking && group.checked >= group.entries_total {
                group.status = GroupStatus::Observed;
                group.diagnostic = None;
            }
            drop(state);
        }
        Err(diagnostic) => {
            mark_failed(shared, target, &diagnostic);
        }
    }
    emit(true);
}

fn mark_failed(shared: &Arc<Mutex<HealthSharedState>>, target: &str, diagnostic: &str) {
    let mut state = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let group = state
        .groups
        .iter_mut()
        .find(|group| group.target_root_id == target)
        .expect("group exists");
    if group.status != GroupStatus::Checking {
        return;
    }
    group.status = GroupStatus::CasFailed;
    group.diagnostic = Some(diagnostic.to_owned());
    for row in &mut group.rows {
        row.observed_state = None;
        row.stale = true;
        row.diagnostic = Some(diagnostic.to_owned());
    }
}

enum EntryOutcome {
    Observed(ActivationObservedState),
    Failed(String),
}

fn classify_entry(
    context: &HealthRunContext,
    entry_path: &std::path::Path,
    expected_target_path: &std::path::Path,
) -> EntryOutcome {
    let snapshot = match context.filesystem.activation_snapshot(entry_path) {
        Ok(snapshot) => snapshot,
        Err(error) => return EntryOutcome::Failed(error.to_string()),
    };
    match snapshot {
        ActivationEntrySnapshot::Missing => {
            EntryOutcome::Observed(ActivationObservedState::Missing)
        }
        ActivationEntrySnapshot::Other => EntryOutcome::Observed(ActivationObservedState::Occupied),
        ActivationEntrySnapshot::Symlink { target } if target != expected_target_path => {
            EntryOutcome::Observed(ActivationObservedState::TargetMismatch)
        }
        ActivationEntrySnapshot::Symlink { .. } => {
            match context
                .filesystem
                .skill_directory_is_readable(expected_target_path)
            {
                Ok(true) => EntryOutcome::Observed(ActivationObservedState::Present),
                Ok(false) => EntryOutcome::Observed(ActivationObservedState::Dangling),
                Err(error) => EntryOutcome::Failed(error.to_string()),
            }
        }
    }
}

/// The CAS (spec §4.10 / ADR-0020): current `home_id`, WriteGate
/// generation, Agent Configuration generation and Target identity must
/// still match the frozen start facts before any observation is persisted.
/// A non-`Open` gate never persists (CatalogReadOnly/Closed keep old);
/// a mismatch keeps the previous durable observation.
fn cas_and_persist(
    context: &HealthRunContext,
    facts: &FrozenHealthFacts,
    target: &str,
    frozen_target: &FrozenTarget,
    observations: &[ActivationObservation],
) -> Result<(), String> {
    let gate = context.write_gate.snapshot();
    let WriteGateState::Open(home) = &gate.state else {
        return Err("activation health requires WriteGate::Open".into());
    };
    if facts.home_id.as_deref() != Some(home.home_id.0.as_str()) {
        return Err("the Bound Home changed during the health run".into());
    }
    if gate.generation != facts.write_gate_generation {
        return Err("the WriteGate generation changed during the health run".into());
    }
    let current = context
        .agent_store
        .agent_configuration_snapshot()
        .map_err(|error| format!("Agent Configuration unavailable: {error}"))?;
    if current.snapshot_version != facts.agent_configuration_generation {
        return Err("the Agent Configuration generation changed during the health run".into());
    }
    let current_target = current
        .roots
        .iter()
        .find(|root| root.root_id == target)
        .ok_or_else(|| "the Target no longer exists in the Agent Configuration".to_owned())?;
    if current_target.path_identity_key != frozen_target.path_identity_key {
        return Err("the Target path identity changed during the health run".into());
    }
    let canonical_now = match context
        .config_fs
        .probe_root(&current_target.configured_path)
    {
        Ok(AgentRootProbe::Present { canonical_path }) => Some(canonical_path),
        _ => None,
    };
    if canonical_now != frozen_target.canonical_path {
        return Err("the Target identity changed during the health run".into());
    }
    context
        .activation_store
        .record_observations(observations)
        .map(|_| ())
        .map_err(|error| format!("activation observation persist failed: {error}"))
}
