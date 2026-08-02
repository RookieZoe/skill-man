use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{ActivationObservedState, AgentId, AgentKind, SkillId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationContext {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub final_entity_path: PathBuf,
    pub agent_id: AgentId,
    pub agent_name: String,
    pub agent_kind: AgentKind,
    pub agent_skills_path: PathBuf,
    pub desired_enabled: bool,
    pub expected_target_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredAgentPath {
    pub agent_id: AgentId,
    pub skills_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationRecord {
    pub skill_id: SkillId,
    pub agent_id: AgentId,
    pub desired_enabled: bool,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
    pub observed_state: ActivationObservedState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesiredActivation {
    pub skill_id: SkillId,
    pub agent_id: AgentId,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationObservation {
    pub skill_id: SkillId,
    pub agent_id: AgentId,
    pub observed_state: ActivationObservedState,
}

#[derive(Debug, Error)]
pub enum ActivationStoreError {
    #[error("the Activation state could not be read or written: {0}")]
    Unavailable(String),
}

pub trait ActivationStore: Send + Sync {
    fn load(
        &self,
        skill_id: &SkillId,
        agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError>;

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError>;

    fn record(&self, record: ActivationRecord) -> Result<u64, ActivationStoreError>;

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
