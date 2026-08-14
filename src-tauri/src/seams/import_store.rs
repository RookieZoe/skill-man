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

/// A remote Install row: a Managed Skill plus its Remote Binding (schema
/// v6, ADR-0013 §4). Used to insert a new remote Install (Handoff, git
/// Import) and to persist an Update. `source_url` is the canonical
/// repository URL; the parent row is looked up or created from it.
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
    /// The parent identity; a fresh UUID is ignored when a parent with the
    /// same canonical URL already exists (the existing parent is reused).
    pub remote_id: String,
    /// The canonical repository URL (spec §3.4 normalization).
    pub source_url: String,
    /// "HEAD" tracks the remote default branch; otherwise the recorded ref.
    pub requested_ref: String,
    /// The commit the installed content was taken from: the Verification
    /// Anchor at Handoff, the install commit for git Imports, and the
    /// applied commit after each Update.
    pub verification_anchor_commit: String,
    /// True when the anchor is known to be the original install commit
    /// (pinned refs and git Imports); false for tree-matched anchors.
    pub original_commit_known: bool,
    /// Repo-relative Skill directory; empty means the repo root.
    pub skill_path: String,
    /// The lock's provider hash, verified at the anchor; `None` for
    /// migrated rows until the next verified fetch.
    pub provider_hash: Option<String>,
    /// The remote tree hash at the anchor (the "upstream" baseline).
    pub remote_baseline_hash: String,
    /// The current Home entity tree hash.
    pub current_baseline_hash: String,
}

/// The persisted view of a remote Install, joined with its Binding and
/// parent row.
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
    pub remote_id: String,
    /// The canonical repository URL (schema v6 parent).
    pub source_url: String,
    pub requested_ref: String,
    pub verification_anchor_commit: String,
    pub original_commit_known: bool,
    pub skill_path: String,
    pub provider_hash: Option<String>,
    pub remote_baseline_hash: String,
    pub current_baseline_hash: String,
    /// UTC epoch seconds of the last successful update check.
    pub last_checked_at: Option<i64>,
    /// UTC epoch seconds of the last applied update.
    pub last_updated_at: Option<i64>,
}

/// One Remote Source Parent: a stable repository identity (ADR-0013 §4).
/// The canonical URL is unique; aliases are user-confirmed URL spellings of
/// the same repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteParentRecord {
    pub remote_id: String,
    pub canonical_url: String,
    pub created_at: String,
    pub aliases: Vec<String>,
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

    /// Find one parent by its canonical URL (or a confirmed alias).
    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, ImportStoreError>;

    /// Every parent row with its confirmed aliases, for manifest integrity
    /// checks (ADR-0013 §4.3).
    fn load_remote_parents(&self) -> Result<Vec<RemoteParentRecord>, ImportStoreError>;

    /// Record a user-confirmed alias for a parent (spec §4.2). Fails when
    /// the alias is already confirmed for a different parent.
    fn insert_remote_alias(&self, remote_id: &str, alias_url: &str)
    -> Result<(), ImportStoreError>;

    /// Delete the parent row when it has no remaining child bindings;
    /// returns whether the parent was deleted (last-child Remove/Undo
    /// semantics, ADR-0013 §4.2).
    fn delete_remote_parent_if_last_child(&self, remote_id: &str)
    -> Result<bool, ImportStoreError>;
}
