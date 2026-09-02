use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRootFingerprint {
    pub canonical_path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentRootEntryKind {
    Directory,
    File { length: u64 },
    Symlink { target: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRootEntryEvidence {
    pub name: String,
    pub kind: AgentRootEntryKind,
}

/// Immutable path evidence frozen into an Agent Configuration plan. Existing
/// roots bind to their directory identity and immediate occupancy; missing
/// roots bind to the nearest existing ancestor and exact missing suffix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRootInspection {
    pub normalized_path: PathBuf,
    pub fingerprint: Option<AgentRootFingerprint>,
    pub nearest_existing_ancestor: AgentRootFingerprint,
    pub missing_components: Vec<String>,
    pub entries: Vec<AgentRootEntryEvidence>,
    pub writable: bool,
}

impl AgentRootInspection {
    pub fn exists(&self) -> bool {
        self.fingerprint.is_some()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedAgentTargetDirectory {
    pub created: Vec<AgentRootFingerprint>,
}

#[derive(Debug, Error)]
pub enum AgentConfigurationFileSystemError {
    #[error("the configured path is not a safe absolute UTF-8 path")]
    InvalidPath,
    #[error("the configured path is not a directory")]
    NotDirectory,
    #[error("the configured path is unavailable: {0}")]
    Unavailable(String),
    #[error("the configured path changed after planning")]
    PlanStale,
    #[error("the configured Target could not be created: {0}")]
    Create(String),
    #[error("the operation-created Target could not be rolled back safely: {0}")]
    Rollback(String),
}

pub trait AgentConfigurationFileSystem: Send + Sync {
    fn inspect_root(
        &self,
        configured_path: &Path,
    ) -> Result<AgentRootInspection, AgentConfigurationFileSystemError>;

    fn create_target(
        &self,
        planned: &AgentRootInspection,
    ) -> Result<CreatedAgentTargetDirectory, AgentConfigurationFileSystemError>;

    fn rollback_created_target(
        &self,
        receipt: &CreatedAgentTargetDirectory,
    ) -> Result<(), AgentConfigurationFileSystemError>;

    fn random_bytes(&self, buffer: &mut [u8]) -> Result<(), AgentConfigurationFileSystemError>;
}
