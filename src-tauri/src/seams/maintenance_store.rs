use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{Health, SkillId};
use crate::seams::activation_store::ActivationStore;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledSkillBaseline {
    pub skill_id: SkillId,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptedSkillEntity {
    pub skill_id: SkillId,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillHealthObservation {
    pub skill_id: SkillId,
    pub health: Health,
}

#[derive(Debug, Error)]
pub enum MaintenanceStoreError {
    #[error("the Maintenance state could not be read or written: {0}")]
    Unavailable(String),
}

pub trait MaintenanceStore: ActivationStore {
    fn installed_skill_baselines(
        &self,
    ) -> Result<Vec<InstalledSkillBaseline>, MaintenanceStoreError>;

    fn record_skill_health(
        &self,
        observations: &[SkillHealthObservation],
    ) -> Result<u64, MaintenanceStoreError>;

    /// Every Managed Skill row (id, final entity, recorded hash if any) used
    /// to decide whether an interrupted Adopt item had committed its catalog
    /// write before the process died.
    fn adopted_skill_entities(&self) -> Result<Vec<AdoptedSkillEntity>, MaintenanceStoreError>;
}
