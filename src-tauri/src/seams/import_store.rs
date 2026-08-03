use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::SkillId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryConflict {
    pub skill_id: SkillId,
    pub directory_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkImportRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub final_entity_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum ImportStoreError {
    #[error("the Library already contains Managed Skill '{0}'")]
    Conflict(String),
    #[error("the Import state could not be read or written: {0}")]
    Unavailable(String),
}

pub trait ImportStore: Send + Sync {
    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<LibraryConflict>, ImportStoreError>;

    fn insert_link(&self, record: LinkImportRecord) -> Result<u64, ImportStoreError>;
}
