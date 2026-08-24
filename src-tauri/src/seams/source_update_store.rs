//! Read-only current facts for a complete Git Repository Source Update.
//!
//! The Core never accepts a member list or prior release from the client.
//! This seam returns one frozen, all-members source snapshot which is then
//! compared with a freshly discovered Source Group Preview.

use std::path::PathBuf;

use thiserror::Error;

use crate::core::domain::{Health, SkillId};
use crate::seams::source_promotion_store::{
    SourcePromotionActivationRecord, SourcePromotionRecord,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateMember {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub skill_path: String,
    pub current_baseline_hash: String,
    pub final_entity_path: PathBuf,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub library_entry_path: PathBuf,
    pub recorded_content_hash: String,
    pub health: Health,
    pub remote_baseline_hash: String,
    pub activations: Vec<SourcePromotionActivationRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateCurrentSource {
    pub remote_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub tracking_ref: String,
    pub current_release_id: String,
    pub current_resolved_commit: String,
    pub aliases: Vec<String>,
    pub created_at: String,
    pub members: Vec<SourceUpdateMember>,
    pub forbidden_local_link_roots: Vec<PathBuf>,
}

#[derive(Debug, Error)]
pub enum SourceUpdateStoreError {
    #[error("the Git Repository Source Update conflicts with Catalog state: {0}")]
    Conflict(String),
    #[error("the Git Repository Source Update Catalog state could not be read: {0}")]
    Unavailable(String),
}

/// Current complete source facts, read atomically enough for Core to make a
/// draft. Confirmation re-reads them; later write methods retain the same
/// full-source contract.
pub trait SourceUpdateStore: Send + Sync {
    fn read_source_update(
        &self,
        remote_id: &str,
    ) -> Result<SourceUpdateCurrentSource, SourceUpdateStoreError>;

    /// The Core passes only its frozen, all-members target record. The
    /// adapter must recheck the complete current release before either the
    /// preflight or transaction commits it.
    fn validate_source_update(
        &self,
        record: &SourcePromotionRecord,
    ) -> Result<(), SourceUpdateStoreError>;

    fn commit_source_update(
        &self,
        record: SourcePromotionRecord,
    ) -> Result<u64, SourceUpdateStoreError>;

    fn source_update_is_committed(
        &self,
        record: &SourcePromotionRecord,
    ) -> Result<bool, SourceUpdateStoreError>;
}
