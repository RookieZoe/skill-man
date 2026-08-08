use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivationEntrySnapshot {
    Missing,
    Symlink { target: PathBuf },
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DirectoryFingerprint {
    pub canonical_path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillFingerprint {
    pub directory: DirectoryFingerprint,
    pub document_device: u64,
    pub document_inode: u64,
    pub document_length: u64,
    pub document_modified_seconds: i64,
    pub document_modified_nanoseconds: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StagedEntryKind {
    Directory,
    File { length: u64 },
    Symlink { target: PathBuf },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StagedTreeEntry {
    pub relative_path: PathBuf,
    pub kind: StagedEntryKind,
    pub device: u64,
    pub inode: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StagedTreeSnapshot {
    pub root: DirectoryFingerprint,
    pub entries: Vec<StagedTreeEntry>,
    pub content_hash: String,
    pub total_file_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileReplacement {
    pub final_entity_path: PathBuf,
    pub installed_fingerprint: DirectoryFingerprint,
    pub backup_path: PathBuf,
    pub backup_fingerprint: DirectoryFingerprint,
    pub original_tree_snapshot: StagedTreeSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileImportJournalPhase {
    Planned,
    FileSystemApplied,
    CatalogCommitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileImportJournalItem {
    pub skill_id: String,
    pub final_entity_path: PathBuf,
    pub expected_content_hash: String,
    pub staged_root_fingerprint: DirectoryFingerprint,
    pub replacement_planned: bool,
    #[serde(default)]
    pub replacement_original_tree: Option<StagedTreeSnapshot>,
    pub installed_fingerprint: Option<DirectoryFingerprint>,
    pub replacement: Option<FileReplacement>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileImportJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: FileImportJournalPhase,
    pub staging_operation_root: PathBuf,
    pub staging_fingerprint: DirectoryFingerprint,
    pub items: Vec<FileImportJournalItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportRecoveryBaseline {
    pub skill_id: String,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptJournalKind {
    /// The entity was moved into the Library (file Install).
    Migrate,
    /// The entity stays outside; the Library records a pointer (Link).
    Link,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptAppearanceKind {
    RealDirectory,
    Symlink {
        original_target: PathBuf,
    },
    /// Entry under a shared/legacy scan source; never an Activation target.
    SharedEntry,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AdoptAppearanceStep {
    pub entry_path: PathBuf,
    pub kind: AdoptAppearanceKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AdoptActivationStep {
    pub agent_id: String,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptItemPhase {
    /// Staged in staging/<op>/<name>; nothing applied yet.
    Staged,
    /// The entity was installed at its stable Library path.
    EntityInstalled,
    /// The catalog row was committed.
    CatalogCommitted,
    /// Old appearances were replaced.
    AppearancesApplied,
    Done,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptJournalPhase {
    Planned,
    Applying,
    Committed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AdoptJournalItem {
    pub skill_id: String,
    pub directory_name: String,
    pub kind: AdoptJournalKind,
    pub staged_root: PathBuf,
    pub staged_fingerprint: DirectoryFingerprint,
    pub final_entity_path: PathBuf,
    /// Empty for Link registrations.
    pub recorded_content_hash: String,
    pub original_path: PathBuf,
    pub original_filename: String,
    pub appearances: Vec<AdoptAppearanceStep>,
    pub activations: Vec<AdoptActivationStep>,
    pub phase: AdoptItemPhase,
    pub installed_fingerprint: Option<DirectoryFingerprint>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AdoptJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: AdoptJournalPhase,
    pub staging_operation_root: PathBuf,
    pub staging_fingerprint: DirectoryFingerprint,
    pub items: Vec<AdoptJournalItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkSourceEntryKind {
    Directory,
    Symlink { target: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkSourceHop {
    pub path: PathBuf,
    pub target: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkSourceSnapshot {
    pub entry_path: PathBuf,
    pub directory_name: String,
    pub entry_device: u64,
    pub entry_inode: u64,
    pub entry_kind: LinkSourceEntryKind,
    pub symlink_chain: Vec<LinkSourceHop>,
    pub final_entity_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScannedSkillEntry {
    pub entry_path: PathBuf,
    pub name: String,
    pub kind: LinkSourceEntryKind,
    pub final_entity_path: Option<PathBuf>,
    pub dangling: bool,
}

#[derive(Debug, Error)]
pub enum FileSystemError {
    #[error("{operation} failed for '{}': {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("'{}' is not a directory", path.display())]
    NotDirectory { path: PathBuf },
    #[error("'{}' is not an absolute configured path", path.display())]
    InvalidConfiguredPath { path: PathBuf },
    #[error("'{}' changed after its operation was planned", path.display())]
    PlanStale { path: PathBuf },
    #[error("{operation} requires recovery for '{}': {message}", path.display())]
    RecoveryRequired {
        operation: &'static str,
        path: PathBuf,
        message: String,
    },
}

pub trait FileSystem: Send + Sync {
    fn inspect_link_source(&self, path: &Path) -> Result<LinkSourceSnapshot, FileSystemError>;

    fn canonical_directory(&self, path: &Path) -> Result<PathBuf, FileSystemError>;

    fn normalize_configured_path(&self, path: &Path) -> Result<PathBuf, FileSystemError>;

    fn directory_fingerprint(&self, path: &Path) -> Result<DirectoryFingerprint, FileSystemError>;

    fn activation_snapshot(
        &self,
        entry_path: &Path,
    ) -> Result<ActivationEntrySnapshot, FileSystemError>;

    fn skill_directory_is_readable(&self, path: &Path) -> Result<bool, FileSystemError>;

    fn skill_fingerprint(&self, path: &Path) -> Result<SkillFingerprint, FileSystemError>;

    fn read_skill_document(&self, path: &Path) -> Result<String, FileSystemError>;

    fn tree_hash(&self, path: &Path) -> Result<String, FileSystemError>;

    fn staged_tree_snapshot(&self, path: &Path) -> Result<StagedTreeSnapshot, FileSystemError>;

    fn available_space(&self, path: &Path) -> Result<u64, FileSystemError>;

    fn staged_child_directories(&self, path: &Path) -> Result<Vec<PathBuf>, FileSystemError>;

    fn staged_has_skill_document(
        &self,
        directory: &Path,
        filename: &str,
    ) -> Result<bool, FileSystemError>;

    fn canonicalize_staged_path(&self, path: &Path) -> Result<PathBuf, FileSystemError>;

    fn install_staged_skill(
        &self,
        staged_skill_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected_staged_tree: &StagedTreeSnapshot,
    ) -> Result<DirectoryFingerprint, FileSystemError>;

    fn discard_staging(
        &self,
        staging_operation_root: &Path,
        library_root: &Path,
        expected: Option<&DirectoryFingerprint>,
    ) -> Result<(), FileSystemError>;

    fn discard_installed_skill(
        &self,
        final_entity_path: &Path,
        library_root: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError>;

    fn replace_staged_skill(
        &self,
        staged_skill_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected_staged_tree: &StagedTreeSnapshot,
        expected_existing_tree: &StagedTreeSnapshot,
    ) -> Result<FileReplacement, FileSystemError> {
        let _ = (
            staged_skill_path,
            final_entity_path,
            library_root,
            operation_id,
            expected_staged_tree,
            expected_existing_tree,
        );
        Err(FileSystemError::Io {
            operation: "replace staged Skill",
            path: final_entity_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "stable replacement is not supported by this filesystem",
            ),
        })
    }

    fn commit_replaced_skill(
        &self,
        replacement: &FileReplacement,
        library_root: &Path,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "commit replaced Skill",
            path: replacement.backup_path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "stable replacement is not supported by this filesystem",
            ),
        })
    }

    fn rollback_replaced_skill(
        &self,
        replacement: &FileReplacement,
        library_root: &Path,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "roll back replaced Skill",
            path: replacement.final_entity_path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "stable replacement is not supported by this filesystem",
            ),
        })
    }

    fn write_file_import_journal(
        &self,
        library_root: &Path,
        journal: &FileImportJournal,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "write file Import journal",
            path: PathBuf::from(&journal.operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journals are not supported by this filesystem",
            ),
        })
    }

    fn finish_file_import_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "finish file Import journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journals are not supported by this filesystem",
            ),
        })
    }

    fn recover_file_import_journals(
        &self,
        library_root: &Path,
        baselines: &[FileImportRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let _ = baselines;
        Err(FileSystemError::Io {
            operation: "recover file Import journals",
            path: library_root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journal recovery is not supported by this filesystem",
            ),
        })
    }

    fn create_activation(
        &self,
        target_path: &Path,
        entry_path: &Path,
    ) -> Result<(), FileSystemError>;

    fn remove_activation(&self, entry_path: &Path) -> Result<(), FileSystemError>;

    /// List the top-level entries of an Agent skills directory for the
    /// Adopt scan: real directories and symlinks (resolved with the same
    /// loop/depth guards as Link sources); dangling entries are reported
    /// with `dangling = true` and no final entity. Files are skipped.
    fn scan_skills_directory(&self, path: &Path)
    -> Result<Vec<ScannedSkillEntry>, FileSystemError>;

    /// Move a real (non-symlink) directory from an external scan source into
    /// the staging root: same-volume rename, cross-volume copy with per-file
    /// verification then delete. Returns the staged fingerprint.
    fn stage_external_directory(
        &self,
        source: &Path,
        staging_destination: &Path,
    ) -> Result<DirectoryFingerprint, FileSystemError>;

    /// Reverse of `stage_external_directory` for Undo: move the directory
    /// back to its original entry path. The destination must be absent.
    fn restore_external_directory(
        &self,
        source: &Path,
        destination: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError>;

    /// Replace the old appearance entries (removing verified symlinks) and
    /// create every planned Activation; idempotent so interrupted Adopt
    /// operations can continue forward during recovery.
    fn apply_adopt_appearances(
        &self,
        appearances: &[AdoptAppearanceStep],
        activations: &[AdoptActivationStep],
    ) -> Result<(), FileSystemError>;

    fn write_adopt_journal(
        &self,
        library_root: &Path,
        journal: &AdoptJournal,
    ) -> Result<(), FileSystemError>;

    fn finish_adopt_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError>;

    fn recover_adopt_journals(
        &self,
        library_root: &Path,
        baselines: &[FileImportRecoveryBaseline],
        adopted_entities: &[FileImportRecoveryBaseline],
    ) -> Result<u32, FileSystemError>;
}
