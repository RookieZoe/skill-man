use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivationEntrySnapshot {
    Missing,
    Symlink { target: PathBuf },
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

    fn create_activation(
        &self,
        target_path: &Path,
        entry_path: &Path,
    ) -> Result<(), FileSystemError>;

    fn remove_activation(&self, entry_path: &Path) -> Result<(), FileSystemError>;
}
