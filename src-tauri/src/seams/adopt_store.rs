use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{AgentId, AgentKind, Health, SkillId};
use crate::seams::import_store::RemoteParentRecord;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptAgent {
    pub agent_id: AgentId,
    pub root_id: String,
    pub name: String,
    pub kind: AgentKind,
    pub skills_path: PathBuf,
    pub activation_target: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptedActivation {
    pub target_root_id: String,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
}

/// One Adopt transaction: the Skill row, its source row (file Install) and
/// every planned Activation are written atomically. `library_entry_path` is
/// `None` for registered Links (the entity stays outside the Library).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptedSkillRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub library_entry_path: Option<PathBuf>,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: Option<String>,
    pub original_path: Option<PathBuf>,
    pub original_filename: String,
    pub activations: Vec<AdoptedActivation>,
}

/// One Remote Install written by the Ownership Handoff (schema v6,
/// ADR-0013 §4–§5): the Skill row, its parent, its Binding and every
/// planned Activation are committed atomically after the lock-entry CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteAdoptedSkillRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: String,
    /// The parent identity; a fresh UUID is ignored when a parent with the
    /// same canonical URL already exists (the existing parent is reused).
    pub remote_id: String,
    pub canonical_url: String,
    pub requested_ref: String,
    pub verification_anchor_commit: String,
    pub original_commit_known: bool,
    pub skill_path: String,
    pub provider_hash: Option<String>,
    pub remote_baseline_hash: String,
    pub current_baseline_hash: String,
    /// Healthy when the Home entity equals the remote baseline (KeepCurrent
    /// with matching trees, DiscardToAnchor); Modified otherwise.
    pub health: Health,
    pub activations: Vec<AdoptedActivation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryConflict {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub final_entity_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum AdoptStoreError {
    #[error("the Library already contains Managed Skill '{0}'")]
    Conflict(String),
    #[error("the Adopt state could not be read or written: {0}")]
    Unavailable(String),
}

pub trait AdoptStore: Send + Sync {
    fn list_agents(&self) -> Result<Vec<AdoptAgent>, AdoptStoreError>;

    fn insert_adopted(&self, record: AdoptedSkillRecord) -> Result<u64, AdoptStoreError>;

    fn remove_adopted_skill(&self, skill_id: &SkillId) -> Result<u64, AdoptStoreError>;

    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<LibraryConflict>, AdoptStoreError>;

    /// Commit one Remote Install (Handoff): parent upsert, Skill row,
    /// Binding and Activations in one transaction. Idempotent: a skill row
    /// that already exists is left untouched and the current snapshot
    /// version is returned (crash roll-forward).
    fn insert_remote_adopted(
        &self,
        record: RemoteAdoptedSkillRecord,
    ) -> Result<u64, AdoptStoreError>;

    /// Delete the parent row when it has no remaining child bindings;
    /// returns whether the parent was deleted (last-child Undo semantics,
    /// ADR-0013 §4.2).
    fn delete_remote_parent_if_last_child(&self, remote_id: &str) -> Result<bool, AdoptStoreError>;

    /// Find one parent by its canonical URL (or a confirmed alias), for
    /// Handoff parent reuse (ADR-0013 §4.2).
    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, AdoptStoreError>;

    /// The binding's parent id for a managed remote Install; `None` when
    /// the skill has no binding (Undo last-child cleanup).
    fn binding_remote_id(&self, skill_id: &SkillId) -> Result<Option<String>, AdoptStoreError>;
}
