use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::SkillId;
use crate::seams::activation_store::DesiredActivation;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub library_entry_path: PathBuf,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: String,
    pub original_path: PathBuf,
    pub original_filename: String,
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

    fn insert_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError>;

    fn insert_files(&self, records: Vec<FileImportRecord>) -> Result<u64, ImportStoreError>;

    fn load_file_install(
        &self,
        identity_key: &str,
    ) -> Result<Option<FileImportRecord>, ImportStoreError>;

    fn desired_activations_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<DesiredActivation>, ImportStoreError>;

    fn replace_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError>;
}
