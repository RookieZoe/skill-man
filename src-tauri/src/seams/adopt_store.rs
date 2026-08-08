use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{AgentId, AgentKind, SkillId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptAgent {
    pub agent_id: AgentId,
    pub name: String,
    pub kind: AgentKind,
    pub skills_path: PathBuf,
    pub detected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptedActivation {
    pub agent_id: AgentId,
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
}
