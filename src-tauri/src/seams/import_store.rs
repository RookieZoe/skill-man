use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{Health, SkillId};
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

/// A remote Install row: a Managed Skill plus its `remote_sources` record.
/// Used both to insert a new remote Install and to persist an Update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteImportRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub library_entry_path: PathBuf,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: String,
    pub source_url: String,
    /// "HEAD" tracks the remote default branch; otherwise the recorded ref.
    pub requested_ref: String,
    pub resolved_commit: String,
    /// Repo-relative Skill directory; empty means the repo root.
    pub skill_path: String,
}

/// The persisted view of a remote Install, joined with its source record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteInstallRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: String,
    pub health: Health,
    pub source_url: String,
    pub requested_ref: String,
    pub resolved_commit: String,
    pub skill_path: String,
    /// UTC epoch seconds of the last successful update check.
    pub last_checked_at: Option<i64>,
    /// UTC epoch seconds of the last applied update.
    pub last_updated_at: Option<i64>,
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

    fn insert_remotes(&self, records: Vec<RemoteImportRecord>) -> Result<u64, ImportStoreError>;

    fn load_remote_installs(&self) -> Result<Vec<RemoteInstallRecord>, ImportStoreError>;

    fn load_remote_install(
        &self,
        identity_key: &str,
    ) -> Result<Option<RemoteInstallRecord>, ImportStoreError>;

    fn update_remote_install(&self, record: RemoteImportRecord) -> Result<u64, ImportStoreError>;

    fn record_remote_check(&self, skill_id: &SkillId) -> Result<(), ImportStoreError>;

    fn set_remote_requested_ref(
        &self,
        skill_id: &SkillId,
        requested_ref: &str,
    ) -> Result<(), ImportStoreError>;
}
