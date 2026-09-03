//! Read-only Legacy Per-Skill Git State facts and the atomic Promotion
//! commit (ADR-0014, spec §8.3).
//!
//! A Promotion is a Source Transition from an unambiguous Legacy parent. It
//! re-fetches a complete Source Release through the same policy flow; old
//! per-Skill refs, commits, anchors and baselines are audit/conflict
//! evidence only and are never published as current release truth. No
//! rename/mapping is guessed: a Legacy member is Current only when its
//! `skill_path` exactly matches a discovered member; everything else is
//! Added or Removed.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::domain::{ActivationObservedState, Health, SkillId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LegacySourcePromotionRecord {
    pub remote_id: String,
    pub canonical_url: String,
    pub aliases: Vec<String>,
    pub created_at: String,
    /// The single legacy tracking ref (audit only; the new release is
    /// discovered by policy/override).
    pub tracking_ref: String,
    /// Present only when the record represents a managed Source Update. A
    /// Legacy Source Promotion has no current Source Release to freeze.
    #[serde(default)]
    pub current_release_id: Option<String>,
    pub members: Vec<LegacySourcePromotionMemberRecord>,
    /// Home, configured Agent/shared skills roots, and installer-managed
    /// roots are never stable Local Link destinations.
    pub forbidden_local_link_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LegacySourcePromotionMemberRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub skill_path: String,
    pub current_baseline_hash: String,
    pub final_entity_path: PathBuf,
    /// Frozen old-Skill facts. These are audit/conflict evidence only and
    /// are never republished as Source Release facts.
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub library_entry_path: PathBuf,
    pub recorded_content_hash: String,
    pub health: Health,
    pub requested_ref: String,
    pub verification_anchor_commit: String,
    pub original_commit_known: bool,
    pub provider_hash: Option<String>,
    pub remote_baseline_hash: String,
    pub last_checked_at: Option<i64>,
    pub last_updated_at: Option<i64>,
    pub activations: Vec<SourcePromotionActivationRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionActivationRecord {
    pub target_root_id: String,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
    pub desired_enabled: bool,
    pub observed_state: ActivationObservedState,
    pub last_enabled_at: Option<String>,
    pub last_checked_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePromotionMemberOrigin {
    /// Reuses the legacy skill_id and keeps its identity.
    Legacy,
    /// A fresh member of the discovered release.
    New,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionMemberRecord {
    pub origin: SourcePromotionMemberOrigin,
    pub skill_id: SkillId,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    /// `<Home>/skills/git/<remote_id>/<skill_id>`.
    pub storage_relpath: String,
    pub skill_path: String,
    pub tree_hash: String,
    pub provider_hash: Option<String>,
    /// Legacy facts frozen for Undo when `origin` is `Legacy`.
    #[serde(default)]
    pub legacy_entity: Option<LegacySourcePromotionMemberRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionRemovedMemberRecord {
    pub skill_id: SkillId,
    pub directory_name: String,
    /// Legacy audit facts frozen for Undo.
    pub legacy_entity: LegacySourcePromotionMemberRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionRecord {
    /// Existing stable parent id. A Promotion must never allocate another.
    pub remote_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub tracking_mode: String,
    pub tracking_value: Option<String>,
    pub selection_kind: String,
    pub selected_ref: String,
    pub release_id: String,
    pub resolved_commit: String,
    /// The immutable operation id also keys the durable legacy audit.
    pub operation_id: String,
    /// Frozen Legacy facts rechecked at Catalog commit. They remain journal
    /// audit only; nothing is copied into current release truth.
    pub legacy: LegacySourcePromotionRecord,
    /// Exact legacy binding ids observed before filesystem work begins.
    pub legacy_member_ids: Vec<SkillId>,
    /// The complete target Source Release.
    pub members: Vec<SourcePromotionMemberRecord>,
    /// Every legacy member absent from the target release.
    pub removed_members: Vec<SourcePromotionRemovedMemberRecord>,
}

#[derive(Debug, Error)]
pub enum SourcePromotionStoreError {
    #[error("the selected Legacy Per-Skill Git State is not promotable: {0}")]
    Conflict(String),
    #[error("the Legacy Per-Skill Git State could not be read: {0}")]
    Unavailable(String),
}

/// The promotion entry point accepts only a durable parent id. The adapter
/// proves that it is still a Legacy parent with a single ref and a complete
/// per-Skill member set; Core never accepts legacy parent/member facts from
/// a client DTO.
pub trait SourcePromotionStore: Send + Sync {
    fn read_legacy_source_promotion(
        &self,
        remote_id: &str,
    ) -> Result<LegacySourcePromotionRecord, SourcePromotionStoreError>;

    /// Preflight and atomically commit one full Source Promotion. The final
    /// transaction rechecks the exact Legacy member set, retains `remote_id`,
    /// creates the repository/release facts, and converts every source member
    /// together. No individual legacy binding can be promoted alone.
    fn validate_source_promotion(
        &self,
        record: &SourcePromotionRecord,
    ) -> Result<(), SourcePromotionStoreError>;

    fn commit_source_promotion(
        &self,
        record: SourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError>;

    /// Exact whole-source state probe used by crash recovery to choose a
    /// direction without re-discovering a newer Git ref.
    fn source_promotion_is_committed(
        &self,
        record: &SourcePromotionRecord,
    ) -> Result<bool, SourcePromotionStoreError>;

    /// Restore the frozen Legacy catalog state only when this exact
    /// Promotion is still current. The caller has already journaled the
    /// source-level Undo cursor and guarded all filesystem state.
    fn undo_source_promotion(
        &self,
        record: &SourcePromotionRecord,
        legacy: &LegacySourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError>;
}
