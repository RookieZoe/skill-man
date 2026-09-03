//! Observation types shared across Startup Probe and Activation Health
//! (spec §4.10; ADR-0020): bounded snapshot summaries, generation-bound
//! page rows and the typed page read failures. Snapshot payloads carry only
//! counts — the full detail is served by `observation_page`, never by a
//! single DTO.

use std::path::PathBuf;

use crate::core::domain::ActivationObservedState;
use crate::core::observation::RootDetectionState;

/// Lifecycle of one observation run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationStatus {
    /// No observation has ever been produced.
    Unknown,
    /// A run is active; the view is being refreshed.
    Checking,
    /// The latest run finished; the view is current for this generation.
    Observed,
    /// Only persisted (cross-startup or kept-old) observations are shown.
    Stale,
}

/// One Startup Probe row: a configured Global Skills Root or Activation
/// Target (existence / readability / path identity observed read-only).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupProbeRow {
    pub configured_path: PathBuf,
    /// The configuration's normalized path identity (never nulled).
    pub path_identity_key: String,
    /// Resolved canonical path when `Present`.
    pub canonical_path: Option<PathBuf>,
    pub state: RootDetectionState,
    /// Raw reason when `Unavailable`; never user copy.
    pub diagnostic: Option<String>,
    pub is_target: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StartupProbeRootCounts {
    pub total: u64,
    pub present: u64,
    pub unavailable: u64,
    pub absent: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StartupProbeTargetCounts {
    pub total: u64,
    pub present: u64,
    pub unavailable: u64,
    pub absent: u64,
}

/// Bounded Startup Probe summary (spec §4.10 `startup_probe`): probing
/// writes nothing, produces no Report/Coverage/Adopt eligibility and never
/// truncates or downgrades on a slow filesystem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupProbeSnapshot {
    pub generation: u64,
    pub status: ObservationStatus,
    pub root_counts: StartupProbeRootCounts,
    pub target_counts: StartupProbeTargetCounts,
    /// Detection + Startup Probe over 1 s: Slow only, never truncated.
    pub slow: bool,
    /// Raw reason when the Agent Configuration snapshot was unreadable;
    /// never user copy.
    pub diagnostic: Option<String>,
}

/// One Activation health row (an enabled `(Skill, Target)` activation).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationHealthRow {
    pub skill_id: String,
    pub target_root_id: String,
    pub entry_path: PathBuf,
    /// The durable expected entity path (internal check evidence).
    pub expected_target_path: PathBuf,
    /// This run's observation; `None` = Unknown (kept old, not fresh).
    pub observed_state: Option<ActivationObservedState>,
    /// The previously persisted observation; `None` when never observed.
    pub previous_state: Option<ActivationObservedState>,
    /// `true` when this row is not fresh for the current generation
    /// (cross-startup, isolated or CAS-failed).
    pub stale: bool,
    /// Raw failure reason when not fresh; never user copy.
    pub diagnostic: Option<String>,
    /// Epoch ms of the last (persisted) observation.
    pub checked_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActivationHealthCounts {
    pub target_groups: u64,
    /// Groups whose rows are fresh for this generation (persisted).
    pub observed_groups: u64,
    /// Groups kept old (CAS mismatch / persist failure).
    pub failed_groups: u64,
    /// Groups isolated by the zero-progress watchdog.
    pub unresponsive_groups: u64,
    pub entries_total: u64,
    pub entries_present: u64,
    /// missing | target_mismatch | dangling | occupied.
    pub entries_unhealthy: u64,
    /// No fresh observation for the current generation.
    pub entries_unknown: u64,
}

/// Bounded Activation Health summary (spec §4.10 `activation_health`).
/// The first screen shows the persisted observations as `Stale/Checking`;
/// Targets are observed and persisted per-group only after the CAS passes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationHealthSnapshot {
    pub generation: u64,
    pub status: ObservationStatus,
    pub target_group_counts: ActivationHealthCounts,
    /// Activation health over 5 s: Slow only, never truncated.
    pub slow: bool,
    /// Raw run-level reason (load failure); never user copy.
    pub diagnostic: Option<String>,
}

/// The observation page contract (spec §4.10 `observation_page`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationKind {
    StartupProbe,
    ActivationHealth,
}

/// Stable cursor into one immutable generation: the row set is fixed at
/// generation start, so an offset cursor over the deterministic ordering
/// never silently moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationCursor {
    pub offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservationRow {
    StartupProbe(StartupProbeRow),
    ActivationHealth(ActivationHealthRow),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationPageRead {
    pub rows: Vec<ObservationRow>,
    /// Continuation offset; `None` when the section is exhausted.
    pub next_offset: Option<u64>,
}

/// Typed page read failures: a generation-bound cursor never silently falls
/// back to another observation generation (spec §4.10).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservationPageError {
    /// The cursor's generation is no longer current.
    Stale { current_generation: u64 },
    /// The kind has never been observable (closed store / no data).
    NotFound,
}

impl ObservationPageError {
    /// `Some(snapshot)` when the failure carries the current generation.
    pub fn stale_message(&self) -> Option<u64> {
        match self {
            Self::Stale { current_generation } => Some(*current_generation),
            Self::NotFound => None,
        }
    }
}
