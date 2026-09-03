//! v9 Git Repository Source lifecycle facts (ADR-0018, ticket #93).
//!
//! One complete source snapshot: source identity, tracking policy, the
//! immutable current Source Release and every current/absent member. The
//! Core never accepts a member list, prior release or storage path from the
//! client; this seam re-reads the frozen facts and the adapters re-check
//! them at every commit.
//!
//! A Source Update is a source-level operation: it freezes the exact
//! previous facts (for validation and Undo), the complete target Source
//! Release and the removed/presence transitions. Member add/remove/reappear
//! is always the outcome of an Update; members never have independent
//! versions. `commit_source_update` is one SQLite transaction and the only
//! Catalog commit point of the update; individual members are never
//! committed alone.

use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{Health, SkillId};

/// A Source Member's presence within the managed source (ADR-0018):
/// `Absent` is the minimal Source Member Tombstone — the stable
/// `(remote_id, skill_path, skill_id)` facts stay until the whole source is
/// removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceMemberPresence {
    Current,
    Absent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateCurrentMember {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub skill_path: String,
    /// `<Home>/skills/git/<remote_id>/<skill_id>` (spec §3.4, ADR-0018).
    pub storage_relpath: String,
    pub presence: SourceMemberPresence,
    /// The current Source Release tree hash; `None` for an absent member.
    pub tree_hash: Option<String>,
    pub health: Health,
    pub display_name: String,
    pub description: String,
    pub last_seen_release_id: String,
}

/// The read-only current facts of one managed v9 Source, including
/// tombstoned members and the frozen current Source Release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateCurrentSource {
    pub remote_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub aliases: Vec<String>,
    pub tracking_mode: String,
    pub tracking_value: Option<String>,
    pub selected_ref: String,
    pub current_release_id: String,
    pub resolved_commit: String,
    pub created_at: String,
    pub members: Vec<SourceUpdateCurrentMember>,
    /// Home, configured Agent/shared skills roots and installer-managed
    /// roots are never stable Local Source Copy destinations.
    pub forbidden_local_link_roots: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceUpdateMemberOrigin {
    /// Reuses the stable skill_id of a (current or tombstoned) member.
    Existing,
    /// A fresh member of the target release; gets a new stable skill_id.
    New,
}

/// One member of the frozen complete target Source Release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateMemberRecord {
    pub origin: SourceUpdateMemberOrigin,
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub storage_relpath: String,
    pub skill_path: String,
    pub tree_hash: String,
    pub provider_hash: Option<String>,
}

/// A current member absent from the target release. The snapshot is
/// deleted and the row becomes the member tombstone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateRemovedMemberRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub skill_path: String,
    pub storage_relpath: String,
    /// Current Source Release tree hash, frozen for Undo/guard checks.
    pub previous_tree_hash: String,
}

/// The exact pre-Update Catalog state, frozen for validation and Undo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdatePreviousMemberRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub skill_path: String,
    pub storage_relpath: String,
    pub presence: SourceMemberPresence,
    pub tree_hash: Option<String>,
    pub health: Health,
}

/// Frozen one-source Update commit. The adapters re-check the complete
/// previous facts before the transaction commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateRecord {
    pub remote_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub tracking_mode: String,
    pub tracking_value: Option<String>,
    pub selection_kind: String,
    pub selected_ref: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub operation_id: String,
    pub previous_release_id: String,
    pub previous_tracking_mode: String,
    pub previous_tracking_value: Option<String>,
    pub previous_selected_ref: String,
    pub previous_resolved_commit: String,
    /// Exact pre-Update member set (current and tombstoned), for validation
    /// and unconditional Undo.
    pub previous_members: Vec<SourceUpdatePreviousMemberRecord>,
    /// The complete target Source Release.
    pub members: Vec<SourceUpdateMemberRecord>,
    /// Every previously-current member absent from the target release.
    pub removed_members: Vec<SourceUpdateRemovedMemberRecord>,
}

/// One registered Create Local Source Copy (ADR-0018).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalSourceCopyRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub final_entity_path: PathBuf,
}

/// One desired Activation entry of a member that the source Remove must
/// remove together with the membership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRemoveActivationFacts {
    pub skill_id: SkillId,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
}

/// Frozen whole-source Remove facts, read before the catalog commit so the
/// filesystem work can be journaled first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRemoveFacts {
    pub remote_id: String,
    pub canonical_url: String,
    pub member_skill_ids: Vec<SkillId>,
    /// Every desired Activation entry of every member.
    pub activations: Vec<SourceRemoveActivationFacts>,
}

#[derive(Debug, Error)]
pub enum SourceUpdateStoreError {
    #[error("Git Repository Source conflict: {0}")]
    Conflict(String),
    #[error("the Git Repository Source facts could not be read or written: {0}")]
    Unavailable(String),
}

/// The complete-source lifecycle seam. The Core passes only durable ids and
/// its frozen record; the adapters re-check the whole source state.
pub trait SourceUpdateStore: Send + Sync {
    /// Complete current facts of a managed v9 Source by stable `remote_id`.
    /// `None` is turned into a Conflict by callers; the returned facts
    /// include tombstoned members and the frozen current Source Release.
    fn read_current(
        &self,
        remote_id: &str,
    ) -> Result<Option<SourceUpdateCurrentSource>, SourceUpdateStoreError>;

    /// Every managed v9 source id (startup verification iteration).
    fn source_ids(&self) -> Result<Vec<String>, SourceUpdateStoreError>;

    /// `(remote_id, health)` of the source member owning `skill_id`; `None`
    /// for a non-Git skill (no snapshot gate).
    fn member_health(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<(String, Health)>, SourceUpdateStoreError>;

    /// Pre-commit validation: live Catalog state still exactly equals the
    /// frozen previous facts and the target release does not collide.
    fn validate_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<(), SourceUpdateStoreError>;

    /// One source-level transaction: publish the target release, update the
    /// source facts, flip presence and setters. Returns the new snapshot
    /// version.
    fn commit_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError>;

    /// True when the source, its current release and every member row still
    /// exactly equal the frozen record. Startup recovery and Undo use it
    /// before any Catalog mutation.
    fn source_update_is_committed(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<bool, SourceUpdateStoreError>;

    /// Undo the frozen update: one transaction restores source facts,
    /// release rows, member presence, health and New-member skill rows to
    /// the exact previous state. Only succeeds while the record still
    /// matches live state exactly.
    fn undo_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError>;

    /// Persist member health after a snapshot re-verification. The Core
    /// decides; the store only applies `(skill_id, health)` atomically.
    fn set_source_member_health(
        &self,
        remote_id: &str,
        health: &[(SkillId, Health)],
    ) -> Result<u64, SourceUpdateStoreError>;

    /// Register a Create Local Source Copy as a Local Source. Directory
    /// Identity conflicts are source-aware: only non-Git Managed Skills
    /// block; Git members with the same name may coexist.
    fn register_local_copy(
        &self,
        record: &LocalSourceCopyRecord,
    ) -> Result<u64, SourceUpdateStoreError>;

    /// Frozen whole-source Remove facts (member ids + desired Activation
    /// entries) for the filesystem journal.
    fn source_remove_facts(
        &self,
        remote_id: &str,
    ) -> Result<SourceRemoveFacts, SourceUpdateStoreError>;

    /// Remove the complete Git Repository Source in one transaction.
    /// Member skill rows (and their activations) are deleted; tombstones
    /// live only until this point.
    fn commit_remove_source(&self, remote_id: &str) -> Result<u64, SourceUpdateStoreError>;

    /// True when the source row is gone (idempotence probe for recovery).
    fn source_remove_is_committed(&self, remote_id: &str) -> Result<bool, SourceUpdateStoreError>;
}
