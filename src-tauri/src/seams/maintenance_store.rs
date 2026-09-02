use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{Health, SkillId, SourceKind};
use crate::seams::activation_store::ActivationStore;
use crate::seams::filesystem::ActivationRecoveryBaseline;
use crate::seams::import_store::RemoteImportRecord;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledSkillBaseline {
    pub skill_id: SkillId,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: String,
}

/// Every Managed Skill row used by the health check: Install entities are
/// compared against their recorded content hash (Modified vs Broken),
/// Link pointers are probed for readability (Broken).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedSkillBaseline {
    pub skill_id: SkillId,
    pub source_kind: SourceKind,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: Option<String>,
}

/// The persisted pointer of one Link Skill; the Relocate flow validates and
/// replaces it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkSkillRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub final_entity_path: PathBuf,
}

/// One desired Activation that must be rewritten when a Link is relocated
/// or removed when its Skill leaves the Library.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelocateActivationBaseline {
    pub target_root_id: String,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
}

/// The persisted identity of a Managed Skill targeted by Remove.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveTarget {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub source_kind: SourceKind,
    pub final_entity_path: PathBuf,
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

/// The complete Catalog write a crash roll-forward needs after the lock
/// CAS: one Skill row, its parent/Binding and every desired Activation
/// (spec §8.4 step 5). The insert is idempotent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandoffRecoveredRecord {
    pub skill: RemoteImportRecord,
    pub activations: Vec<ActivationRecoveryBaseline>,
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

    /// Every Managed Skill (Link and Install) with its persisted health
    /// inputs; the health check recomputes Broken/Modified from the
    /// filesystem, never trusting the persisted value alone.
    fn managed_skill_baselines(&self) -> Result<Vec<ManagedSkillBaseline>, MaintenanceStoreError> {
        self.installed_skill_baselines().map(|installed| {
            installed
                .into_iter()
                .map(|baseline| ManagedSkillBaseline {
                    skill_id: baseline.skill_id,
                    source_kind: SourceKind::RemoteInstall,
                    final_entity_path: baseline.final_entity_path,
                    recorded_content_hash: Some(baseline.recorded_content_hash),
                })
                .collect()
        })
    }

    /// The persisted Link pointer to relocate; `None` when the Skill is not
    /// a Link or does not exist.
    fn link_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<LinkSkillRecord>, MaintenanceStoreError>;

    /// Desired Activations of one Skill; the Relocate flow rewrites their
    /// symlinks to the new final entity, and Remove deletes them.
    fn activation_baselines_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<RelocateActivationBaseline>, MaintenanceStoreError> {
        self.desired_activation_baselines().map(|activations| {
            activations
                .into_iter()
                .filter(|activation| activation.skill_id == skill_id.0)
                .map(|activation| RelocateActivationBaseline {
                    target_root_id: activation.target_root_id,
                    expected_entry_path: activation.expected_entry_path,
                    expected_target_path: activation.expected_target_path,
                })
                .collect()
        })
    }

    /// Commit a relocation in one transaction: move the Link pointer to the
    /// new entity, refresh display metadata and health, and repoint every
    /// desired Activation at the new target (observed Present, because the
    /// symlinks were already rewritten). Returns the new snapshot version.
    fn commit_relocate(
        &self,
        skill_id: &SkillId,
        final_entity_path: PathBuf,
        display_name: String,
        description: String,
        new_target_path: PathBuf,
        activations: &[RelocateActivationBaseline],
    ) -> Result<u64, MaintenanceStoreError>;

    /// The persisted identity of a Managed Skill targeted by Remove; `None`
    /// when the Skill does not exist.
    fn remove_target(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<RemoveTarget>, MaintenanceStoreError>;

    /// Delete the Skill row in one transaction; activations and source
    /// tables cascade with the row. Returns the new snapshot version.
    fn delete_skill(&self, skill_id: &SkillId) -> Result<u64, MaintenanceStoreError>;

    /// The Binding's parent id of a managed remote Install, captured before
    /// Remove deletes the Skill row (last-child parent cleanup).
    fn binding_remote_id(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<String>, MaintenanceStoreError>;

    /// Delete the parent row when it has no remaining child bindings;
    /// returns whether the parent was deleted (last-child Remove).
    fn delete_remote_parent_if_last_child(
        &self,
        remote_id: &str,
    ) -> Result<bool, MaintenanceStoreError>;

    /// Crash roll-forward of a committed Handoff: parent upsert, Skill
    /// row, Binding and Activations in one idempotent transaction.
    fn insert_handoff_recovered(
        &self,
        record: HandoffRecoveredRecord,
    ) -> Result<u64, MaintenanceStoreError>;

    /// Find one parent by its canonical URL (or a confirmed alias), for
    /// Handoff roll-forward manifest backfill.
    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<crate::seams::import_store::RemoteParentRecord>, MaintenanceStoreError>;

    fn record_skill_health(
        &self,
        observations: &[SkillHealthObservation],
    ) -> Result<u64, MaintenanceStoreError>;

    /// Every Managed Skill row (id, final entity, recorded hash if any) used
    /// to decide whether an interrupted Adopt item had committed its catalog
    /// write before the process died.
    fn adopted_skill_entities(&self) -> Result<Vec<AdoptedSkillEntity>, MaintenanceStoreError>;

    /// Every desired Activation; used to decide whether an interrupted
    /// Remove-then-replace had committed its catalog write before the process
    /// died. The default (no baselines) makes recovery roll uncommitted
    /// replaces back — the safe direction for stores without Activation data.
    fn desired_activation_baselines(
        &self,
    ) -> Result<Vec<ActivationRecoveryBaseline>, MaintenanceStoreError> {
        Ok(Vec::new())
    }
}
