//! Atomic Catalog persistence for one complete Git Repository Source release
//! (ADR-0018). The transition service owns filesystem staging, external
//! ownership and its journal. This seam owns only the Catalog transaction
//! that makes every release/member fact current together after the Source
//! Ownership Commit Point.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

use crate::core::domain::SkillId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransitionTarget {
    pub root_id: String,
    pub path: PathBuf,
}

/// Previous global use, frozen before isolation; never inferred on recovery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceTransitionActivation {
    pub skill_id: String,
    pub target_root_id: String,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
    pub parent_fingerprint: crate::seams::filesystem::DirectoryFingerprint,
    /// None means the original entry was the isolated real directory.
    pub previous_target: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransitionMemberRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    /// `<Home>/skills/git/<remote_id>/<skill_id>` (spec §3.4, ADR-0018).
    pub storage_relpath: String,
    pub skill_path: String,
    pub tree_hash: String,
    pub provider_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransitionRecord {
    pub activations: Vec<SourceTransitionActivation>,
    pub remote_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub aliases: Vec<String>,
    pub tracking_mode: String,
    pub tracking_value: Option<String>,
    pub selection_kind: String,
    pub selected_ref: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub members: Vec<SourceTransitionMemberRecord>,
}

/// One current member of an already-managed v9 Source (used by recovery and
/// the source-already-managed guard).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExistingSourceMember {
    pub skill_id: String,
    pub directory_name: String,
    pub skill_path: String,
}

/// The read-only current facts of one managed v9 Source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExistingSourceFacts {
    pub remote_id: String,
    pub canonical_url: String,
    pub members: Vec<ExistingSourceMember>,
}

#[derive(Debug, Error)]
pub enum SourceTransitionStoreError {
    #[error("the Source Transition conflicts with existing Catalog state: {0}")]
    Conflict(String),
    #[error("the Source Transition Catalog state could not be read or written: {0}")]
    Unavailable(String),
}

/// One source-level SQLite transaction. It creates the source, immutable
/// release facts, all Managed Skills and all current-member rows together;
/// individual members never carry a ref or commit.
pub trait SourceTransitionStore: Send + Sync {
    fn activation_targets(&self)
    -> Result<Vec<SourceTransitionTarget>, SourceTransitionStoreError>;

    /// Separate from release identity: later Enable/Disable is not an
    /// uncommitted Source Release.
    fn transition_activations_match(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<bool, SourceTransitionStoreError>;
    /// Current v9 members of a managed source identified by canonical URL.
    /// `None` when no v9 Git Repository Source owns the canonical URL; a
    /// Legacy parent (v7 bindings) is also `None` here.
    fn existing_current_members(
        &self,
        canonical_url: &str,
    ) -> Result<Option<Vec<ExistingSourceMember>>, SourceTransitionStoreError>;

    /// Current members of a managed v9 Source identified by its stable
    /// `remote_id`; `None` when no v9 source owns it.
    fn existing_source(
        &self,
        remote_id: &str,
    ) -> Result<Option<ExistingSourceFacts>, SourceTransitionStoreError>;

    /// Prove that a new, whole-source commit would not collide with an
    /// existing Source or Managed Skill. This runs before external ownership
    /// is released; the final commit repeats the checks in its transaction.
    fn validate_new_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<(), SourceTransitionStoreError>;

    fn commit_source_transition(
        &self,
        record: SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError>;

    /// True only when this exact immutable Source Release is the source's
    /// current release and every frozen member still has its matching
    /// current-member row. Startup recovery uses this before deciding whether
    /// it must finish the post-CAS Catalog commit.
    fn source_transition_is_committed(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<bool, SourceTransitionStoreError>;

    /// Remove a newly-created source only when its current release and whole
    /// member set still exactly equal `record`. This is deliberately a
    /// source-level operation: a Source Undo can never delete one member.
    fn undo_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError>;
}
