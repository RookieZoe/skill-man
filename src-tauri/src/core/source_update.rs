//! Source-wide Git Repository Source Update.
//!
//! Updates deliberately reuse the Source Group Draft, the staged filesystem
//! transition, and the durable fixed-release journal from Source Promotion.
//! The adapter below changes only the Catalog side: it proves an already
//! managed source is still whole, then atomically advances that source.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::core::source_group_preview::SourceGroupPreviewService;
use crate::core::source_promotion::{
    ConfirmSourcePromotionRequest, SourcePromotionError, SourcePromotionResult,
    SourcePromotionService,
};
use crate::core::write_gate::WriteGate;
use crate::seams::clock::Clock;
use crate::seams::filesystem::FileSystem;
use crate::seams::installer_lock_store::InstallerLockStore;
use crate::seams::source::GitSource;
use crate::seams::source_promotion_store::{
    LegacySourcePromotionMemberRecord, LegacySourcePromotionRecord, SourcePromotionStore,
    SourcePromotionStoreError,
};
use crate::seams::source_update_store::{SourceUpdateStore, SourceUpdateStoreError};

pub use crate::core::source_promotion::{
    ModifiedMemberResolution, SourcePromotionDraft as SourceUpdateDraft,
    SourcePromotionMemberState as SourceUpdateMemberState,
    SourcePromotionResolution as SourceUpdateResolution, UpstreamMemberRemovedResolution,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmSourceUpdateRequest {
    pub remote_id: String,
    pub expected_resolved_commit: String,
    pub resolutions: Vec<SourceUpdateResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateResult {
    pub operation_id: String,
    pub remote_id: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub member_count: u32,
    pub snapshot_version: u64,
}

#[derive(Debug, Error)]
pub enum SourceUpdateError {
    #[error("the Source Group Draft is stale; preview the Source Update again")]
    PlanStale,
    #[error("Ownership Conflict: an external installer claims this Git Repository Source")]
    OwnershipConflict,
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Store(#[from] SourceUpdateStoreError),
    #[error("Source Update recovery is required: {0}")]
    RecoveryRequired(String),
}

/// Public Core seam. One `remote_id` is always read back to a complete
/// current source; callers cannot provide a selected subset of members.
pub struct SourceUpdateService {
    inner: SourcePromotionService,
}

impl SourceUpdateService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        preview: Arc<SourceGroupPreviewService>,
        git_source: Arc<dyn GitSource>,
        store: Arc<dyn SourceUpdateStore>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        library_root: PathBuf,
        home_directory: PathBuf,
    ) -> Self {
        let adapted: Arc<dyn SourcePromotionStore> = Arc::new(SourceUpdatePromotionStore { store });
        Self {
            inner: SourcePromotionService::new_source_update(
                preview,
                git_source,
                adapted,
                filesystem,
                clock,
                library_root,
            )
            .with_external_owner_roots(vec![home_directory.join(".agents/skills")]),
        }
    }

    pub fn with_lock_store(mut self, lock_store: Arc<dyn InstallerLockStore>) -> Self {
        self.inner = self.inner.with_lock_store(lock_store);
        self
    }

    pub fn with_write_gate(mut self, write_gate: Arc<WriteGate>) -> Self {
        self.inner = self.inner.with_write_gate(write_gate);
        self
    }

    pub fn with_home_context(mut self, home_context: Arc<WriteGate>) -> Self {
        self.inner = self.inner.with_home_context(home_context);
        self
    }

    pub fn preview(&self, remote_id: &str) -> Result<SourceUpdateDraft, SourceUpdateError> {
        self.inner.preview(remote_id).map_err(map_error)
    }

    pub fn confirm(
        &self,
        request: ConfirmSourceUpdateRequest,
    ) -> Result<SourceUpdateResult, SourceUpdateError> {
        self.inner
            .confirm(ConfirmSourcePromotionRequest {
                remote_id: request.remote_id,
                expected_resolved_commit: request.expected_resolved_commit,
                resolutions: request.resolutions,
            })
            .map(map_result)
            .map_err(map_error)
    }

    pub fn finalize(&self, operation_id: &str) -> Result<(), SourceUpdateError> {
        self.inner.finalize(operation_id).map_err(map_error)
    }

    /// Startup-only recovery follows the same frozen whole-source journal as
    /// confirmation. It is deliberately separate from Legacy Source
    /// Promotion recovery so each mode ignores the other's operation ids.
    pub fn recover_pending(&self, library_root: &Path) -> Result<(), SourceUpdateError> {
        self.inner.recover_pending(library_root).map_err(map_error)
    }
}

fn map_result(result: SourcePromotionResult) -> SourceUpdateResult {
    SourceUpdateResult {
        operation_id: result.operation_id,
        remote_id: result.remote_id,
        release_id: result.release_id,
        resolved_commit: result.resolved_commit,
        member_count: result.member_count,
        snapshot_version: result.snapshot_version,
    }
}

fn map_error(error: SourcePromotionError) -> SourceUpdateError {
    match error {
        SourcePromotionError::OwnershipConflict => SourceUpdateError::OwnershipConflict,
        SourcePromotionError::Validation(message)
            if message.contains("Source Group Draft is stale")
                || message.contains("discovered release changed") =>
        {
            SourceUpdateError::PlanStale
        }
        SourcePromotionError::RecoveryRequired(message) => {
            SourceUpdateError::RecoveryRequired(message)
        }
        SourcePromotionError::Store(error) => match error {
            SourcePromotionStoreError::Conflict(message) => {
                SourceUpdateError::Store(SourceUpdateStoreError::Conflict(message))
            }
            SourcePromotionStoreError::Unavailable(message) => {
                SourceUpdateError::Store(SourceUpdateStoreError::Unavailable(message))
            }
        },
        other => SourceUpdateError::Validation(other.to_string()),
    }
}

struct SourceUpdatePromotionStore {
    store: Arc<dyn SourceUpdateStore>,
}

impl SourcePromotionStore for SourceUpdatePromotionStore {
    fn read_legacy_source_promotion(
        &self,
        remote_id: &str,
    ) -> Result<LegacySourcePromotionRecord, SourcePromotionStoreError> {
        let current = self
            .store
            .read_source_update(remote_id)
            .map_err(map_store_error)?;
        Ok(LegacySourcePromotionRecord {
            remote_id: current.remote_id,
            canonical_url: current.canonical_url,
            aliases: current.aliases,
            created_at: current.created_at,
            tracking_ref: current.tracking_ref.clone(),
            current_release_id: Some(current.current_release_id),
            members: current
                .members
                .into_iter()
                .map(|member| LegacySourcePromotionMemberRecord {
                    skill_id: member.skill_id,
                    directory_name: member.directory_name,
                    skill_path: member.skill_path,
                    current_baseline_hash: member.current_baseline_hash,
                    final_entity_path: member.final_entity_path,
                    identity_key: member.identity_key,
                    display_name: member.display_name,
                    description: member.description,
                    library_entry_path: member.library_entry_path,
                    recorded_content_hash: member.recorded_content_hash,
                    health: member.health,
                    requested_ref: current.tracking_ref.clone(),
                    verification_anchor_commit: current.current_resolved_commit.clone(),
                    original_commit_known: true,
                    provider_hash: None,
                    remote_baseline_hash: member.remote_baseline_hash,
                    last_checked_at: None,
                    last_updated_at: None,
                    activations: member.activations,
                })
                .collect(),
            forbidden_local_link_roots: current.forbidden_local_link_roots,
        })
    }

    fn validate_source_promotion(
        &self,
        record: &crate::seams::source_promotion_store::SourcePromotionRecord,
    ) -> Result<(), SourcePromotionStoreError> {
        self.store
            .validate_source_update(record)
            .map_err(map_store_error)
    }

    fn commit_source_promotion(
        &self,
        record: crate::seams::source_promotion_store::SourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError> {
        self.store
            .commit_source_update(record)
            .map_err(map_store_error)
    }

    fn source_promotion_is_committed(
        &self,
        record: &crate::seams::source_promotion_store::SourcePromotionRecord,
    ) -> Result<bool, SourcePromotionStoreError> {
        self.store
            .source_update_is_committed(record)
            .map_err(map_store_error)
    }

    fn undo_source_promotion(
        &self,
        _record: &crate::seams::source_promotion_store::SourcePromotionRecord,
        _legacy: &LegacySourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError> {
        Err(SourcePromotionStoreError::Conflict(
            "Source Update has no implicit Source Undo; an ordinary Remove never restores an external owner"
                .into(),
        ))
    }
}

fn map_store_error(error: SourceUpdateStoreError) -> SourcePromotionStoreError {
    match error {
        SourceUpdateStoreError::Conflict(message) => SourcePromotionStoreError::Conflict(message),
        SourceUpdateStoreError::Unavailable(message) => {
            SourcePromotionStoreError::Unavailable(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_catalog_unavailable_across_the_reused_promotion_adapter() {
        let error = map_error(SourcePromotionError::Store(
            SourcePromotionStoreError::Unavailable("SQLite lock poisoned".into()),
        ));

        assert!(matches!(
            error,
            SourceUpdateError::Store(SourceUpdateStoreError::Unavailable(message))
                if message == "SQLite lock poisoned"
        ));
    }
}
