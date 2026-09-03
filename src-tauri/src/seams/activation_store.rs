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

#[derive(Debug, Error)]
pub enum ActivationStoreError {
    #[error("the Activation state could not be read or written: {0}")]
    Unavailable(String),
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
}
