//! Atomic Catalog persistence for one complete Git Repository Source release.
//!
//! The transition service owns filesystem staging, external ownership and its
//! journal. This seam owns only the Catalog transaction that makes every
//! release/member fact current together after the Source Ownership Commit
//! Point.

use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::SkillId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransitionMemberRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub library_entry_path: PathBuf,
    pub final_entity_path: PathBuf,
    pub skill_path: String,
    pub tree_hash: String,
    pub provider_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransitionRecord {
    /// Generated before the journal is written. A pre-existing canonical URL
    /// is a closed conflict here: source-level Update belongs to ticket #60.
    pub remote_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub tracking_ref: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub members: Vec<SourceTransitionMemberRecord>,
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
