use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{ActivationObservedState, SkillId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesiredActivation {
    pub skill_id: SkillId,
    pub target_root_id: String,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationObservation {
    pub skill_id: SkillId,
    pub target_root_id: String,
    pub observed_state: ActivationObservedState,
}

/// One persisted activation row with its last durable observation (ADR-0020
/// Activation Health Observation): the health module loads these at
/// generation start so the first screen can show `Stale/Checking` old
/// observations before a Target is re-observed, and keeps the old value
/// when a Target is isolated or CAS fails.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredActivationObservation {
    pub skill_id: SkillId,
    pub target_root_id: String,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
    /// `None` when the row has never been observed (unknown).
    pub observed_state: Option<ActivationObservedState>,
    /// Epoch milliseconds of the last durable observation.
    pub last_checked_at_ms: Option<u64>,
}

/// One Activation row in full, including disabled rows: the Enable Module
/// (spec §4.9) plans and recovers per `(Skill, Target, Directory Identity)`
/// cells from this fact set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationCellRow {
    pub skill_id: SkillId,
    pub target_root_id: String,
    pub directory_identity_key: String,
    pub desired_enabled: bool,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
    pub observed_state: Option<ActivationObservedState>,
    /// Epoch milliseconds (ISO stored as text; parsed for tests).
    pub last_enabled_at_ms: Option<u64>,
}

/// The desired state of one cell, written by the Enable Module as the
/// operation commit point (spec §4.9). All writes of one cell commit in one
/// catalog transaction; enabled cells carry the exact expected paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationCellWrite {
    pub skill_id: SkillId,
    pub target_root_id: String,
    pub directory_identity_key: String,
    pub desired_enabled: bool,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum ActivationStoreError {
    #[error("the Activation state could not be read or written: {0}")]
    Unavailable(String),
    /// The `(Target, Directory Identity)` unique ownership of an enabled
    /// entry was contested at commit time (spec §4.9; ADR-0019).
    #[error("the Activation entry '{destination}' is owned by another Skill")]
    EntryConflict { destination: String },
}

pub trait ActivationStore: Send + Sync {
    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError>;

    /// Every desired (`desired_enabled = 1`) activation with its persisted
    /// observation (read-only; works for Catalog ReadOnly).
    fn activation_observations(
        &self,
    ) -> Result<Vec<StoredActivationObservation>, ActivationStoreError>;

    fn record_observations(
        &self,
        observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError>;

    fn record_observation(
        &self,
        observation: &ActivationObservation,
    ) -> Result<u64, ActivationStoreError> {
        self.record_observations(std::slice::from_ref(observation))
    }

    /// Every Activation row, enabled and disabled (read-only; works for
    /// Catalog ReadOnly). The recovery facts of the Enable journal derive
    /// from this set.
    fn activation_cells(&self) -> Result<Vec<ActivationCellRow>, ActivationStoreError>;

    /// Every Activation row of one Skill (enabled and disabled).
    fn activation_cells_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<ActivationCellRow>, ActivationStoreError>;

    /// Persist exactly these cell desired states in one catalog transaction
    /// (the Enable commit point), bumping `snapshot_version`; returns the
    /// new Catalog generation.
    fn write_activation_cells(
        &self,
        writes: &[ActivationCellWrite],
    ) -> Result<u64, ActivationStoreError>;

    /// The current Catalog generation (`snapshot_version`) the Enable plan
    /// freezes and re-verifies before apply.
    fn catalog_generation(&self) -> Result<u64, ActivationStoreError>;
}
