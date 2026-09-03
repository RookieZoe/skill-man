//! Remote verification seam (spec §4.6, ADR-0013 §2.2): GitHub, GitLab and
//! generic HTTPS Git providers that parse a lock entry into a normalized
//! request, resolve the Verification Anchor per moving/pinned ref rules,
//! check the provider-specific hash and materialize the anchor tree. The
//! seam never writes Home; mirrors and materialized trees live in a caller
//! workspace the caller discards.
//!
//! `Conflict` errors mean the lock facts contradict the remote (Provenance
//! Conflict); `Deferred` errors mean availability facts (DNS/TLS/timeout/
//! rate limit/401/403/404) prevented a closed loop (Verification Deferred).
//! A fetch failure is never silently downgraded to Local.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::seams::installer_lock_store::LockEntry;

/// The audited provider kinds (ADR-0013 §2.2.4): unknown, download, local,
/// well-known or algorithmically unclear types are never masqueraded as Git.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteKind {
    Github,
    Gitlab,
    GenericGit,
}

/// A normalized verification request parsed from a lock entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteRequest {
    pub kind: RemoteKind,
    pub canonical_url: String,
    /// The lock's ref, or `HEAD` when the installer recorded none (spec
    /// §8.2: a missing requested ref tracks the remote default branch).
    pub requested_ref: String,
    /// The repository-relative Skill directory (empty or "." = repo root).
    pub skill_path: String,
    /// The lock's `skillFolderHash`, interpreted per provider kind.
    pub provider_hash: String,
}

/// How the requested ref resolves inside the fetched mirror.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefDisposition {
    /// No ref recorded; the remote default branch is tracked.
    Head,
    /// A branch ref: moving rules apply (tip match, then ancestry search).
    Branch,
    /// A tag ref: pinned rules apply, exact match only.
    Tag,
    /// A raw commit ref: pinned rules apply, exact match only.
    Commit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorResolution {
    pub anchor_commit: String,
    /// True only for pinned refs (tag/commit): the lock identifies the exact
    /// install commit. Anchors found by tree matching never claim the
    /// installer's unrecorded original commit (ADR-0013 §2.2).
    pub original_install_commit_known: bool,
}

/// One provider Release fact for Source Tracking Policy evaluation
/// (ADR-0018). Providers without a formal Release concept report an empty
/// list; the policy then falls back to tags.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderReleaseFact {
    pub tag_name: String,
    pub is_prerelease: bool,
    pub is_draft: bool,
    /// Provider publish time, epoch nanoseconds; `None` sorts as oldest.
    pub published_at: Option<u128>,
}

/// The provider facts the Core combines into a closed-loop verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteTreeFacts {
    pub disposition: RefDisposition,
    pub anchor: AnchorResolution,
    /// Git tree SHA of the skillPath subtree at the anchor (the GitHub
    /// provider-hash semantics; recorded for display for every kind).
    pub subtree_tree_sha: String,
    /// Whether the lock's provider hash matched at the anchor (GitHub:
    /// subtree tree SHA; GitLab/generic: the installer CLI's local SHA-256).
    pub provider_hash_matched: bool,
    /// The materialized `anchor + skillPath` tree; the caller hashes it with
    /// `tree-sha256-v1` and MUST discard the workspace afterwards.
    pub materialized_root: PathBuf,
    pub default_branch: Option<String>,
}

#[derive(Debug, Error)]
pub enum RemoteProviderError {
    /// The lock fields contradict, embed credentials, or use an unsafe
    /// ref/path/type, or the remote contradicts the lock after a successful
    /// fetch (Provenance Conflict).
    #[error("remote source validation failed: {0}")]
    Conflict(String),
    /// Availability facts prevented the closed loop (Verification
    /// Deferred); never silently downgrades to Local.
    #[error("remote verification deferred: {0}")]
    Deferred(String),
}

/// GitHub / GitLab / generic HTTPS Git verification behind one seam.
pub trait RemoteProvider: Send + Sync {
    /// Parse and normalize a lock entry into a verification request;
    /// rejects unknown source types, embedded credentials, contradictory
    /// source fields and unsafe refs/paths (ADR-0013 §2.2.3–§2.2.6).
    fn parse_request(&self, entry: &LockEntry) -> Result<RemoteRequest, RemoteProviderError>;

    /// Fetch the remote into `workspace` (mirror keyed by canonical URL),
    /// resolve the Verification Anchor per moving/pinned rules, verify the
    /// provider hash at the anchor, and materialize the anchor tree. All
    /// failures are `Conflict` (facts) or `Deferred` (availability); nothing
    /// here writes Home or the lock.
    fn verify(
        &self,
        request: &RemoteRequest,
        workspace: &Path,
    ) -> Result<RemoteTreeFacts, RemoteProviderError>;

    /// The provider's formal Releases for `canonical_url`, newest first is
    /// not required (the policy sorts deterministically). Generic Git
    /// providers have no releases and return an empty list; a provider
    /// outage is a `Deferred` availability fact and never a silent fallback.
    fn list_releases(
        &self,
        canonical_url: &str,
    ) -> Result<Vec<ProviderReleaseFact>, RemoteProviderError> {
        let _ = canonical_url;
        Ok(Vec::new())
    }
}

/// Fail-closed default: no remote verification can fabricate evidence until
/// the composition root wires the system adapter.
pub struct UnavailableRemoteProvider;

impl RemoteProvider for UnavailableRemoteProvider {
    fn parse_request(&self, _entry: &LockEntry) -> Result<RemoteRequest, RemoteProviderError> {
        Err(RemoteProviderError::Conflict(
            "no remote provider is configured".into(),
        ))
    }

    fn verify(
        &self,
        _request: &RemoteRequest,
        _workspace: &Path,
    ) -> Result<RemoteTreeFacts, RemoteProviderError> {
        Err(RemoteProviderError::Conflict(
            "no remote provider is configured".into(),
        ))
    }
}
