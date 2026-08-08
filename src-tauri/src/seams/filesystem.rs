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
        let _ = (library_root, baselines);
        Ok(0)
    }

    fn create_activation(
        &self,
        target_path: &Path,
        entry_path: &Path,
    ) -> Result<(), FileSystemError>;

    fn remove_activation(&self, entry_path: &Path) -> Result<(), FileSystemError>;
}
