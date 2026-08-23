use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedFileSource {
    pub original_path: PathBuf,
    pub original_filename: String,
    pub suggested_root_name: String,
    pub staged_content_root: PathBuf,
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("{0}")]
    Validation(String),
    #[error("{operation} failed for '{}': {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Git {0}")]
    Git(String),
}

pub trait FileSource: Send + Sync {
    fn estimated_size(&self, source_path: &Path) -> Result<u64, SourceError>;

    fn stage(
        &self,
        source_path: &Path,
        staging_root: &Path,
    ) -> Result<StagedFileSource, SourceError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitTreeEntryKind {
    Blob,
    Tree,
    Symlink,
    Submodule,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitTreeEntry {
    pub path: PathBuf,
    pub kind: GitTreeEntryKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitFetchReport {
    /// The remote's default branch name, when the transport could discover it.
    pub default_branch: Option<String>,
}

/// Transport-level Git operations behind the Source seam. The adapter
/// fetches into a mirror the Core owns under `<Library>/cache/git/`;
/// it never writes Library or Agent paths itself.
pub trait GitSource: Send + Sync {
    /// Ensure `mirror_dir` exists as a mirror of `url` (clone on first use,
    /// fetch afterwards) and report the remote's default branch.
    fn fetch_mirror(&self, url: &str, mirror_dir: &Path) -> Result<GitFetchReport, SourceError>;

    /// Resolve `rev` to a commit in the mirror; `None` when it is not present.
    fn resolve_commit(&self, mirror_dir: &Path, rev: &str) -> Result<Option<String>, SourceError>;

    /// List every entry of `commit`'s tree, recursively.
    fn list_tree(&self, mirror_dir: &Path, commit: &str) -> Result<Vec<GitTreeEntry>, SourceError>;

    /// Read one file at `path` in `commit`; `None` when the path is absent.
    fn read_blob(
        &self,
        mirror_dir: &Path,
        commit: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, SourceError>;

    /// Return the immutable Git tree object ID for the whole Skill directory
    /// at `skill_path` (empty = repository root) in `commit`.
    fn tree_summary(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
    ) -> Result<String, SourceError>;

    /// Materialize the Skill directory at `skill_path` (empty = repo root)
    /// of `commit` into `destination` as a regular directory tree.
    fn stage_skill(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
        destination: &Path,
    ) -> Result<(), SourceError>;
}
