//! Local Git worktree probe seam (spec §4.10 concurrency cap; ADR-0017
//! bounded worktree discovery): a zero-network, read-only upward search from
//! an appearance entry to the nearest `.git` inside the originating Root.
//! The result is a *hint* — it never constitutes a Git Repository Source or a
//! Source Release (ADR-0017); classification and source identity are
//! fail-closed later tickets (spec §8.3, #84).
//!
//! Hard rules the probe implements (ADR-0017 §worktree):
//! - the upward walk never crosses above the originating canonical Root;
//!   a Home/dotfiles repository above the Root is ambient and ignored;
//! - `.git` may be a directory or a `gitdir: <target>` file; a target that
//!   is missing/relative-unsafe is typed `uninterpretable`, never guessed;
//! - zero network: only local metadata files are read.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// gitdir shapes the probe distinguishes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitdirKind {
    /// `.git` is a real directory.
    Directory,
    /// `.git` is a `gitdir: <target>` file pointing at `target`.
    Gitfile { target: PathBuf },
    /// `.git` exists but cannot be interpreted (missing/unsafe target,
    /// unreadable metadata). The hint is recorded as `uninterpretable`.
    Invalid,
}

/// One `[remote "…"]` URL fact; URLs are Source Content, never App Copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteUrlEvidence {
    pub name: String,
    pub url: String,
}

/// The typed bounded worktree hint of one appearance (spec §4.10 memoized
/// facts; ADR-0017).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeHint {
    /// The repository root found inside the originating Root.
    pub repository_root: PathBuf,
    pub gitdir_kind: GitdirKind,
    pub remote_urls: Vec<RemoteUrlEvidence>,
    /// `ref: refs/heads/<branch>` value of `HEAD`, or `None` when detached
    /// or unreadable.
    pub head_ref: Option<String>,
    /// Typed reason when the metadata is uninterpretable (never guess).
    pub uninterpretable: Option<String>,
}

impl WorktreeHint {
    pub fn is_valid(&self) -> bool {
        self.uninterpretable.is_none() && self.gitdir_kind != GitdirKind::Invalid
    }
    pub fn gitdir_kind_name(&self) -> &'static str {
        match self.gitdir_kind {
            GitdirKind::Directory => "dir",
            GitdirKind::Gitfile { .. } => "file",
            GitdirKind::Invalid => "uninterpretable",
        }
    }
}

/// No `.git` anywhere between the entry and the originating Root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoWorktree;

#[derive(Debug, Error)]
pub enum LocalGitProbeError {
    #[error("the entry path is outside the originating Root")]
    OutsideRoot,
    #[error("the worktree metatadata could not be read: {0}")]
    Io(String),
}

pub trait LocalGitProbe: Send + Sync {
    /// Zero-network bounded worktree probe for one appearance `entry`; the
    /// upward walk stops at `originating_root` (inclusive). Entries whose
    /// parent chain contains no `.git` yield `Ok(None)`.
    fn probe_worktree(
        &self,
        entry: &Path,
        originating_root: &Path,
    ) -> Result<Option<WorktreeHint>, LocalGitProbeError>;
}
