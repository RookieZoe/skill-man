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

#[derive(Debug, Error)]
pub enum ActivationStoreError {
    #[error("the Activation state could not be read or written: {0}")]
    Unavailable(String),
}

pub trait ActivationStore: Send + Sync {
    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError>;

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
