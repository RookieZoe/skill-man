//! Legacy Source Promotion draft classification.
//!
//! A Legacy Per-Skill Git State has only per-member evidence.  It must never
//! be silently interpreted as a current Source Release.  This module builds
//! the in-memory Source Group Draft that compares those legacy members with a
//! freshly discovered, complete release.  It has no persistence seam: cancel
//! and re-discovery therefore leave Home, Catalog, staging and journals alone.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::domain::{Health, SkillId, parse_skill_metadata, skill_identity_key};
use crate::core::git_source::{git_mirror_path, parse_git_source_input, resolve_git_ref};
use crate::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreviewError, SourceGroupPreviewOutcome,
    SourceGroupPreviewService,
};
use crate::core::source_group_preview::{SourceGroupMember, SourceGroupPreview};
use crate::core::write_gate::WriteGate;
use crate::seams::clock::Clock;
use crate::seams::filesystem::{
    ActivationEntrySnapshot, DirectoryFingerprint, FileReplacement, FileSystem, FileSystemError,
    RemoteParentManifest, StagedTreeSnapshot,
};
use crate::seams::installer_lock_store::{
    EmptyInstallerLockStore, InstallerLockStore, LockFileReport,
};
use crate::seams::source::{GitSource, SourceError};
use crate::seams::source_promotion_store::{
    LegacySourcePromotionRecord, SourcePromotionMemberOrigin, SourcePromotionMemberRecord,
    SourcePromotionRecord, SourcePromotionRemovedMemberRecord, SourcePromotionStore,
    SourcePromotionStoreError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacySourcePromotion {
    pub remote_id: String,
    pub canonical_url: String,
    pub tracking_ref: String,
    pub members: Vec<LegacySourcePromotionMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacySourcePromotionMember {
    pub skill_id: SkillId,
    pub directory_name: String,
    /// The historical per-Skill path.  It is an audit/conflict input only;
    /// the new Source Release always comes from fresh discovery.
    pub skill_path: String,
    pub current_baseline_hash: String,
    /// Computed from the current managed entity immediately before the draft
    /// is returned.  This is the only local-content fact the draft exposes.
    pub current_tree_hash: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModifiedMemberResolution {
    KeepModified,
    ReplaceWithTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpstreamMemberRemovedResolution {
    Remove,
    LocalLink { target_directory: String },
    ExplicitMemberMapping { target_skill_path: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionResolution {
    pub skill_id: SkillId,
    pub modified: Option<ModifiedMemberResolution>,
    pub removed: Option<UpstreamMemberRemovedResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourcePromotionMemberState {
    /// The target release has the same explicit path and current content is
    /// still the recorded local baseline.
    UpdateToTarget,
    /// The target path exists, but confirmation must choose whether to keep
    /// the local bytes as Modified or replace them with target content.
    ModifiedMemberResolutionRequired,
    /// The target release no longer contains the old path.  No rename is
    /// inferred: confirm must Remove, Local Link, or explicitly map it.
    UpstreamMemberRemoved,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionExistingMemberDraft {
    pub member: LegacySourcePromotionMember,
    pub state: SourcePromotionMemberState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionTargetMemberDraft {
    pub member: SourceGroupMember,
    /// `Some` only for an exact historical path match.  A mapping is applied
    /// later by `confirm`; discovery itself never guesses one.
    pub legacy_skill_id: Option<SkillId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionDraft {
    pub remote_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub tracking_ref: String,
    pub resolved_commit: String,
    pub existing_members: Vec<SourcePromotionExistingMemberDraft>,
    pub target_members: Vec<SourcePromotionTargetMemberDraft>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmedSourcePromotion {
    pub draft: SourcePromotionDraft,
    pub resolutions: BTreeMap<String, SourcePromotionResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmSourcePromotionRequest {
    pub remote_id: String,
    pub expected_resolved_commit: String,
    pub resolutions: Vec<SourcePromotionResolution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionResult {
    pub operation_id: String,
    pub remote_id: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub member_count: u32,
    pub snapshot_version: u64,
    pub undo_available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionUndoResult {
    pub operation_id: String,
    pub member_count: u32,
    pub snapshot_version: u64,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SourcePromotionDraftError {
    #[error("the Legacy Source Promotion has no stable remote id")]
    MissingRemoteId,
    #[error("the Legacy Source Promotion has no canonical repository")]
    MissingCanonicalUrl,
    #[error("the Legacy Source Promotion has no single tracking ref")]
    MissingTrackingRef,
    #[error("the Legacy Source Promotion requires at least one legacy member")]
    EmptyLegacyMembers,
    #[error("the Legacy Source Promotion contains duplicate legacy skill path '{0}'")]
    DuplicateLegacyPath(String),
    #[error("the Legacy Source Promotion contains duplicate legacy skill id '{0}'")]
    DuplicateLegacySkillId(String),
    #[error("the discovered Source Release does not match the selected legacy repository")]
    RepositoryMismatch,
    #[error("the discovered Source Release does not match the selected legacy tracking ref")]
    TrackingRefMismatch,
    #[error("the discovered Source Release is incomplete at duplicate skill path '{0}'")]
    DuplicateTargetPath(String),
    #[error("the Source Group Draft is missing a resolution for legacy member '{0}'")]
    MissingResolution(String),
    #[error(
        "the Source Group Draft supplied a resolution for a member that is not conflicted: '{0}'"
    )]
    UnexpectedResolution(String),
    #[error("the Source Group Draft supplied a mismatched member id '{0}'")]
    MismatchedResolution(String),
    #[error("the Source Group Draft must use exactly one resolution for legacy member '{0}'")]
    InvalidResolution(String),
    #[error("the Explicit Member Mapping target '{0}' is not in the discovered Source Release")]
    UnknownMappingTarget(String),
    #[error("the Explicit Member Mapping target '{0}' is already occupied")]
    OccupiedMappingTarget(String),
    #[error("the Local Link target for legacy member '{0}' is empty")]
    EmptyLocalLinkTarget(String),
}

#[derive(Debug, Error)]
pub enum SourcePromotionError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Preview(#[from] SourceGroupPreviewError),
    #[error(transparent)]
    Store(#[from] SourcePromotionStoreError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Draft(#[from] SourcePromotionDraftError),
    #[error("Source Promotion recovery is required: {0}")]
    RecoveryRequired(String),
    #[error("Ownership Conflict: an external installer claims this Git Repository Source")]
    OwnershipConflict,
}

const SOURCE_PROMOTION_JOURNAL_VERSION: u32 = 1;

/// Durable, source-scoped transition state. It intentionally freezes both
/// legacy audit facts and the discovered target release: restart recovery
/// never consults a later remote tip or guesses a legacy member mapping.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
struct SourcePromotionJournal {
    version: u32,
    operation_id: String,
    phase: SourcePromotionPhase,
    staging_operation_root: PathBuf,
    staging_fingerprint: Option<DirectoryFingerprint>,
    legacy: LegacySourcePromotionRecord,
    record: Option<SourcePromotionRecord>,
    previous_manifest: Option<RemoteParentManifest>,
    target_manifest: Option<RemoteParentManifest>,
    mutations: PromotionMutations,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourcePromotionPhase {
    Planned,
    MembersStaged,
    FilesystemApplying,
    FilesystemApplied,
    ManifestWritten,
    CatalogCommitted,
    Finalized,
    Finalizing,
    Undoing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourcePromotionMode {
    LegacyPromotion,
    SourceUpdate,
}

/// The public Core seam for an explicit Source Promotion preview.  It reads
/// one durable Legacy parent, discovers a fresh complete release through the
/// same Source Group Preview service as a clean transition, then classifies
/// conflicts entirely in memory.
pub struct SourcePromotionService {
    preview: Arc<SourceGroupPreviewService>,
    git_source: Arc<dyn GitSource>,
    store: Arc<dyn SourcePromotionStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    configured_library_root: PathBuf,
    home_context: Option<Arc<WriteGate>>,
    write_gate: Arc<WriteGate>,
    lock_store: Arc<dyn InstallerLockStore>,
    external_owner_roots: Vec<PathBuf>,
    mode: SourcePromotionMode,
    next_id: AtomicU64,
}

impl SourcePromotionService {
    pub fn new(
        preview: Arc<SourceGroupPreviewService>,
        git_source: Arc<dyn GitSource>,
        store: Arc<dyn SourcePromotionStore>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        library_root: PathBuf,
    ) -> Self {
        Self {
            preview,
            git_source,
            store,
            filesystem,
            clock,
            configured_library_root: library_root,
            home_context: None,
            write_gate: Arc::new(WriteGate::open_for_tests()),
            lock_store: Arc::new(EmptyInstallerLockStore),
            external_owner_roots: Vec::new(),
            mode: SourcePromotionMode::LegacyPromotion,
            next_id: AtomicU64::new(1),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_source_update(
        preview: Arc<SourceGroupPreviewService>,
        git_source: Arc<dyn GitSource>,
        store: Arc<dyn SourcePromotionStore>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        library_root: PathBuf,
    ) -> Self {
        let mut service = Self::new(preview, git_source, store, filesystem, clock, library_root);
        service.mode = SourcePromotionMode::SourceUpdate;
        service
    }

    pub fn with_write_gate(mut self, write_gate: Arc<WriteGate>) -> Self {
        self.write_gate = write_gate;
        self
    }

    pub fn with_home_context(mut self, home_context: Arc<WriteGate>) -> Self {
        self.home_context = Some(home_context);
        self
    }

    pub fn with_lock_store(mut self, lock_store: Arc<dyn InstallerLockStore>) -> Self {
        self.lock_store = lock_store;
        self
    }

    pub fn with_external_owner_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.external_owner_roots = roots;
        self
    }

    pub fn preview(&self, remote_id: &str) -> Result<SourcePromotionDraft, SourcePromotionError> {
        if remote_id.trim().is_empty() {
            return Err(SourcePromotionError::Validation(
                "Source Promotion requires a Legacy remote id".into(),
            ));
        }
        let (_, draft) = self.read_draft(remote_id)?;
        Ok(draft)
    }

    fn read_draft(
        &self,
        remote_id: &str,
    ) -> Result<(LegacySourcePromotionRecord, SourcePromotionDraft), SourcePromotionError> {
        if remote_id.trim().is_empty() {
            return Err(SourcePromotionError::Validation(
                "Source Promotion requires a Legacy remote id".into(),
            ));
        }
        let record = self.store.read_legacy_source_promotion(remote_id)?;
        let manifest = self.filesystem.read_remote_parent_manifest(
            &self.active_library_root()?.join("remotes"),
            &record.remote_id,
        )?;
        match (self.mode, manifest) {
            (SourcePromotionMode::LegacyPromotion, None) => {
                return Err(SourcePromotionError::Validation(
                    "the selected Legacy Source Promotion is missing its source capability manifest"
                        .into(),
                ));
            }
            (SourcePromotionMode::SourceUpdate, None) => {
                // #63 sources predate the managed-source manifest. Their
                // SQLite current-release facts remain authoritative for this
                // zero-write Preview; confirmation journals and publishes
                // the new manifest with the source-level release.
            }
            (_, Some(manifest)) => {
                let mut catalog_aliases = record.aliases.clone();
                let mut manifest_aliases = manifest.aliases;
                catalog_aliases.sort();
                manifest_aliases.sort();
                let still_legacy = manifest.provider.is_none()
                    && manifest.tracking_ref.is_none()
                    && manifest.current_release_id.is_none();
                let manifest_matches_mode = match self.mode {
                    SourcePromotionMode::LegacyPromotion => still_legacy,
                    SourcePromotionMode::SourceUpdate => {
                        manifest
                            .provider
                            .as_deref()
                            .is_some_and(|value| !value.is_empty())
                            && manifest.tracking_ref.as_deref()
                                == Some(record.tracking_ref.as_str())
                            && manifest.current_release_id == record.current_release_id
                    }
                };
                if manifest.remote_id != record.remote_id
                    || manifest.canonical_url != record.canonical_url
                    || manifest_aliases != catalog_aliases
                    || !manifest_matches_mode
                {
                    return Err(SourcePromotionError::Validation(
                        "the selected Legacy Source Promotion has incomplete or conflicting source capability facts"
                            .into(),
                    ));
                }
            }
        }
        let members = record
            .members
            .clone()
            .into_iter()
            .map(|member| {
                let current_tree_hash = self.filesystem.tree_hash(&member.final_entity_path)?;
                Ok(LegacySourcePromotionMember {
                    skill_id: member.skill_id,
                    directory_name: member.directory_name,
                    skill_path: member.skill_path,
                    // A managed Source Update compares local bytes with the
                    // last remote baseline, not with a prior KeepModified
                    // snapshot. The latter is frozen in `record` for CAS,
                    // but must never make a later Update silently overwrite
                    // those local edits.
                    current_baseline_hash: match self.mode {
                        SourcePromotionMode::LegacyPromotion => member.current_baseline_hash,
                        SourcePromotionMode::SourceUpdate => member.remote_baseline_hash,
                    },
                    current_tree_hash,
                })
            })
            .collect::<Result<Vec<_>, FileSystemError>>()?;
        let legacy = LegacySourcePromotion {
            remote_id: record.remote_id.clone(),
            canonical_url: record.canonical_url.clone(),
            tracking_ref: record.tracking_ref.clone(),
            members,
        };
        let outcome = self
            .preview
            .fetch_latest_and_manage(FetchLatestAndManageRequest {
                source_type: source_type_for(&record.canonical_url).into(),
                source_url: record.canonical_url.clone(),
                tracking_ref: Some(record.tracking_ref.clone()),
            })?;
        let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
            return Err(SourcePromotionError::Validation(
                "a Legacy Source Promotion requires one complete, unambiguous Source Release"
                    .into(),
            ));
        };
        if !preview.external_ownership_claims.is_empty() {
            return Err(match self.mode {
                SourcePromotionMode::LegacyPromotion => SourcePromotionError::Validation(
                    "a Legacy Source Promotion cannot merge external ownership claims".into(),
                ),
                SourcePromotionMode::SourceUpdate => SourcePromotionError::OwnershipConflict,
            });
        }
        if self.mode == SourcePromotionMode::SourceUpdate
            && self.source_update_external_reappeared(
                &record,
                &self
                    .lock_store
                    .discover()
                    .map_err(|error| SourcePromotionError::Validation(error.to_string()))?,
                &[],
            )?
        {
            return Err(SourcePromotionError::OwnershipConflict);
        }
        Ok((record, SourcePromotionDraft::build(legacy, preview)?))
    }

    pub fn confirm(
        &self,
        request: ConfirmSourcePromotionRequest,
    ) -> Result<SourcePromotionResult, SourcePromotionError> {
        self.ensure_writes_ready()?;
        let (legacy_record, draft) = self.read_draft(&request.remote_id)?;
        if draft.resolved_commit != request.expected_resolved_commit {
            return Err(SourcePromotionError::Validation(
                "the Source Group Draft is stale; preview the Legacy Source Promotion again".into(),
            ));
        }
        let confirmed = draft.clone().confirm(request.resolutions)?;
        let library_root = self.active_library_root()?;
        let operation_id = self.next_operation_id();
        let release_id = format!("source-release-{operation_id}");
        // The journal exists before any target member is staged. It freezes
        // the legacy audit record and the operation root; later cursors add
        // the discovered release, explicit actions and physical mutations.
        let mut journal = SourcePromotionJournal {
            version: SOURCE_PROMOTION_JOURNAL_VERSION,
            operation_id: operation_id.clone(),
            phase: SourcePromotionPhase::Planned,
            staging_operation_root: library_root.join("staging").join(&operation_id),
            staging_fingerprint: None,
            legacy: legacy_record.clone(),
            record: None,
            previous_manifest: None,
            target_manifest: None,
            mutations: PromotionMutations::default(),
        };
        self.write_journal(&library_root, &journal)?;
        let staging_fingerprint = self
            .filesystem
            .create_adopt_staging_operation(&library_root, &operation_id)?;
        journal.staging_fingerprint = Some(staging_fingerprint.clone());
        self.advance_after_mutation(
            &library_root,
            &mut journal,
            SourcePromotionPhase::Planned,
            "record Source Promotion staging creation",
        )?;

        let prepared = match self.prepare_target_members(
            &library_root,
            &operation_id,
            &confirmed,
            &legacy_record,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = self.filesystem.discard_staging(
                    &journal.staging_operation_root,
                    &library_root,
                    Some(&staging_fingerprint),
                );
                let _ = self
                    .filesystem
                    .finish_source_promotion_journal(&library_root, &operation_id);
                return Err(error);
            }
        };
        let record = SourcePromotionRecord {
            operation_id: operation_id.clone(),
            legacy: legacy_record.clone(),
            remote_id: draft.remote_id.clone(),
            provider: draft.provider.clone(),
            canonical_url: draft.canonical_url.clone(),
            tracking_ref: draft.tracking_ref.clone(),
            release_id: release_id.clone(),
            resolved_commit: draft.resolved_commit.clone(),
            legacy_member_ids: draft
                .existing_members
                .iter()
                .map(|member| member.member.skill_id.clone())
                .collect(),
            members: prepared
                .members
                .iter()
                .map(|member| member.record.clone())
                .collect(),
            removed_members: prepared.removed_records.clone(),
        };
        if let Err(error) = self.store.validate_source_promotion(&record) {
            let _ = self.filesystem.discard_staging(
                &journal.staging_operation_root,
                &library_root,
                Some(&staging_fingerprint),
            );
            let _ = self
                .filesystem
                .finish_source_promotion_journal(&library_root, &operation_id);
            return Err(error.into());
        }

        let remotes_root = library_root.join("remotes");
        let previous_manifest = self
            .filesystem
            .read_remote_parent_manifest(&remotes_root, &draft.remote_id)?;
        let manifest = RemoteParentManifest {
            schema_version: 1,
            remote_id: draft.remote_id.clone(),
            canonical_url: draft.canonical_url.clone(),
            provider: Some(draft.provider.clone()),
            tracking_ref: Some(draft.tracking_ref.clone()),
            current_release_id: Some(release_id.clone()),
            aliases: legacy_record.aliases.clone(),
            created_at: legacy_record.created_at.clone(),
        };
        journal.record = Some(record.clone());
        journal.previous_manifest = previous_manifest.clone();
        journal.target_manifest = Some(manifest.clone());
        self.advance_after_mutation(
            &library_root,
            &mut journal,
            SourcePromotionPhase::MembersStaged,
            "record Source Promotion staged members",
        )?;

        // `stage_skill` and every other Draft preparation step are
        // intentionally outside the mutation boundary. Recheck immediately
        // before publishing any Source Update bytes so an external installer
        // cannot reappear between the earlier prepare guard and commit.
        if self.mode == SourcePromotionMode::SourceUpdate {
            let reappeared = (|| -> Result<bool, SourcePromotionError> {
                let target_directory_names = draft
                    .target_members
                    .iter()
                    .map(|member| member.member.directory_name.clone())
                    .collect::<Vec<_>>();
                let lock_reports = self
                    .lock_store
                    .discover()
                    .map_err(|error| SourcePromotionError::Validation(error.to_string()))?;
                self.source_update_external_reappeared(
                    &legacy_record,
                    &lock_reports,
                    &target_directory_names,
                )
            })();
            let reappeared = match reappeared {
                Ok(reappeared) => reappeared,
                Err(error) => {
                    let _ = self.filesystem.discard_staging(
                        &journal.staging_operation_root,
                        &library_root,
                        Some(&staging_fingerprint),
                    );
                    let _ = self
                        .filesystem
                        .finish_source_promotion_journal(&library_root, &operation_id);
                    return Err(error);
                }
            };
            if reappeared {
                let _ = self.filesystem.discard_staging(
                    &journal.staging_operation_root,
                    &library_root,
                    Some(&staging_fingerprint),
                );
                let _ = self
                    .filesystem
                    .finish_source_promotion_journal(&library_root, &operation_id);
                return Err(SourcePromotionError::OwnershipConflict);
            }
        }

        journal.phase = SourcePromotionPhase::FilesystemApplying;
        self.write_journal(&library_root, &journal)?;
        if let Err(error) = self.apply_filesystem_changes(
            &library_root,
            &operation_id,
            &prepared,
            &legacy_record,
            &mut journal,
        ) {
            if let Err(rollback_error) =
                self.rollback_filesystem_changes(&library_root, &mut journal.mutations)
            {
                let _ = self.write_journal(&library_root, &journal);
                self.write_gate.mark_blocked();
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "rollback an interrupted Source Promotion: {rollback_error}"
                )));
            }
            if let Err(cleanup_error) = self.finish_rolled_back_promotion(&library_root, &journal) {
                self.write_gate.mark_blocked();
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "clean up an interrupted Source Promotion: {cleanup_error}"
                )));
            }
            return Err(error);
        }
        self.advance_after_mutation(
            &library_root,
            &mut journal,
            SourcePromotionPhase::FilesystemApplied,
            "record Source Promotion filesystem completion",
        )?;
        if let Err(error) = self
            .filesystem
            .write_remote_parent_manifest(&remotes_root, &manifest)
        {
            if let Err(rollback_error) =
                self.rollback_filesystem_changes(&library_root, &mut journal.mutations)
            {
                let _ = self.write_journal(&library_root, &journal);
                self.write_gate.mark_blocked();
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "rollback a Source Promotion manifest write: {rollback_error}"
                )));
            }
            // A manifest write can have completed its rename before the
            // directory fsync reports an error.  Restore the old manifest
            // before retiring the journal, otherwise Legacy Catalog/files
            // could be paired with a published target release.
            if let Err(restore_error) = self.restore_legacy_manifest(
                &remotes_root,
                &draft.remote_id,
                previous_manifest.as_ref(),
            ) {
                self.write_gate.mark_blocked();
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "rollback a Source Promotion manifest write: restore the Legacy manifest: {restore_error}"
                )));
            }
            if let Err(cleanup_error) = self.finish_rolled_back_promotion(&library_root, &journal) {
                self.write_gate.mark_blocked();
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "rollback a Source Promotion manifest write: clean up the durable journal: {cleanup_error}"
                )));
            }
            return Err(error.into());
        }
        self.advance_after_mutation(
            &library_root,
            &mut journal,
            SourcePromotionPhase::ManifestWritten,
            "record Source Promotion manifest publication",
        )?;
        let snapshot_version = match self.store.commit_source_promotion(record) {
            Ok(version) => version,
            Err(error) => {
                if let Err(rollback_error) =
                    self.rollback_filesystem_changes(&library_root, &mut journal.mutations)
                {
                    let _ = self.write_journal(&library_root, &journal);
                    self.write_gate.mark_blocked();
                    return Err(SourcePromotionError::RecoveryRequired(format!(
                        "rollback a Source Promotion Catalog write: {rollback_error}"
                    )));
                }
                if let Err(restore_error) = self.restore_legacy_manifest(
                    &remotes_root,
                    &draft.remote_id,
                    previous_manifest.as_ref(),
                ) {
                    self.write_gate.mark_blocked();
                    return Err(SourcePromotionError::RecoveryRequired(format!(
                        "Source Promotion rolled back but its Legacy source manifest needs recovery: {restore_error}"
                    )));
                }
                if let Err(cleanup_error) =
                    self.finish_rolled_back_promotion(&library_root, &journal)
                {
                    self.write_gate.mark_blocked();
                    return Err(SourcePromotionError::RecoveryRequired(format!(
                        "clean up a rolled-back Source Promotion: {cleanup_error}"
                    )));
                }
                return Err(error.into());
            }
        };
        // Retain all frozen copies until the result is finalized. A source
        // promotion is undoable only as a complete source, never per Skill.
        self.advance_after_mutation(
            &library_root,
            &mut journal,
            SourcePromotionPhase::CatalogCommitted,
            "record Source Promotion Catalog commit",
        )?;
        if let Err(error) = self.verify_final_source(&library_root, &journal) {
            self.write_gate.mark_blocked();
            return Err(error);
        }
        self.advance_after_mutation(
            &library_root,
            &mut journal,
            SourcePromotionPhase::Finalized,
            "record Source Promotion result window",
        )?;
        Ok(SourcePromotionResult {
            operation_id,
            remote_id: draft.remote_id,
            release_id,
            resolved_commit: draft.resolved_commit,
            member_count: u32::try_from(prepared.members.len())
                .map_err(|_| SourcePromotionError::Validation("too many Source Members".into()))?,
            snapshot_version,
            undo_available: true,
        })
    }

    /// Startup-only recovery. The durable journal, not a freshly fetched
    /// ref, determines every decision after an interrupted confirmation.
    pub fn recover_pending(&self, library_root: &Path) -> Result<(), SourcePromotionError> {
        for (operation_id, bytes) in self
            .filesystem
            .list_source_promotion_journals(library_root)?
        {
            if !self.accepts_operation_id(&operation_id) {
                continue;
            }
            let mut journal = self.decode_journal(&operation_id, &bytes)?;
            match journal.phase {
                SourcePromotionPhase::Planned | SourcePromotionPhase::MembersStaged => {
                    if self
                        .filesystem
                        .path_is_occupied(&journal.staging_operation_root)?
                    {
                        self.discard_promotion_staging(library_root, &journal)?;
                    }
                    self.filesystem
                        .finish_source_promotion_journal(library_root, &journal.operation_id)?;
                }
                SourcePromotionPhase::FilesystemApplying
                | SourcePromotionPhase::FilesystemApplied
                | SourcePromotionPhase::ManifestWritten => {
                    let Some(record) = journal.record.as_ref() else {
                        return Err(SourcePromotionError::RecoveryRequired(
                            "the Source Promotion journal is missing its frozen target release"
                                .into(),
                        ));
                    };
                    if self.store.source_promotion_is_committed(record)? {
                        self.verify_final_source(library_root, &journal)?;
                        journal.phase = SourcePromotionPhase::Finalized;
                        self.write_journal(library_root, &journal)?;
                    } else {
                        self.rollback_filesystem_changes(library_root, &mut journal.mutations)?;
                        self.restore_legacy_manifest(
                            &library_root.join("remotes"),
                            &journal.legacy.remote_id,
                            journal.previous_manifest.as_ref(),
                        )?;
                        self.discard_promotion_staging(library_root, &journal)?;
                        self.filesystem
                            .finish_source_promotion_journal(library_root, &journal.operation_id)?;
                    }
                }
                SourcePromotionPhase::CatalogCommitted | SourcePromotionPhase::Finalized => {
                    let Some(record) = journal.record.as_ref() else {
                        return Err(SourcePromotionError::RecoveryRequired(
                            "the committed Source Promotion journal is missing its target release"
                                .into(),
                        ));
                    };
                    if !self.store.source_promotion_is_committed(record)? {
                        return Err(SourcePromotionError::RecoveryRequired(
                            "the Source Promotion Catalog state no longer matches its frozen journal".into(),
                        ));
                    }
                    self.verify_final_source(library_root, &journal)?;
                    if self.mode == SourcePromotionMode::SourceUpdate {
                        // Source Updates have no Undo window. A restart has
                        // no result dialog that can finalize this operation,
                        // so reclaim the frozen staging and backups once the
                        // fixed release is proved current.
                        self.complete_finalization(library_root, &journal)?;
                    } else {
                        journal.phase = SourcePromotionPhase::Finalized;
                        self.write_journal(library_root, &journal)?;
                    }
                }
                SourcePromotionPhase::Finalizing => {
                    self.complete_finalization(library_root, &journal)?;
                }
                SourcePromotionPhase::Undoing => {
                    self.complete_undo(library_root, &mut journal)?;
                }
            }
        }
        Ok(())
    }

    pub fn undo(
        &self,
        operation_id: &str,
    ) -> Result<SourcePromotionUndoResult, SourcePromotionError> {
        self.ensure_writes_ready()?;
        let library_root = self.active_library_root()?;
        let mut journal = self.journal_for(&library_root, operation_id)?;
        if journal.phase != SourcePromotionPhase::Finalized {
            return Err(SourcePromotionError::Validation(
                "the Source Promotion is not in an undoable result window".into(),
            ));
        }
        let Some(record) = journal.record.as_ref() else {
            self.write_gate.mark_blocked();
            return Err(SourcePromotionError::RecoveryRequired(
                "the Source Promotion journal is missing its target release".into(),
            ));
        };
        let member_count = record.members.len();
        match self.store.source_promotion_is_committed(record) {
            Ok(true) => {}
            Ok(false) => {
                self.write_gate.mark_blocked();
                return Err(SourcePromotionError::RecoveryRequired(
                    "the Source Promotion result no longer matches its frozen release".into(),
                ));
            }
            Err(error) => {
                self.write_gate.mark_blocked();
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "verify the Source Promotion result before Undo: {error}"
                )));
            }
        }
        if let Err(error) = self.verify_final_source(&library_root, &journal) {
            self.write_gate.mark_blocked();
            return Err(SourcePromotionError::RecoveryRequired(format!(
                "verify Source Promotion files before Undo: {error}"
            )));
        }
        journal.phase = SourcePromotionPhase::Undoing;
        self.write_journal(&library_root, &journal)?;
        let snapshot_version = match self.complete_undo(&library_root, &mut journal) {
            Ok(version) => version,
            Err(error) => {
                // The Catalog may already be back in Legacy state while an
                // owned entity or manifest still awaits rollback.  Leave the
                // durable cursor for startup recovery and stop all ordinary
                // product writes until that recovery has completed.
                self.write_gate.mark_blocked();
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "complete Source Promotion Undo: {error}"
                )));
            }
        };
        Ok(SourcePromotionUndoResult {
            operation_id: operation_id.into(),
            member_count: u32::try_from(member_count)
                .map_err(|_| SourcePromotionError::Validation("too many Source Members".into()))?,
            snapshot_version,
        })
    }

    pub fn finalize(&self, operation_id: &str) -> Result<(), SourcePromotionError> {
        self.ensure_writes_ready()?;
        let library_root = self.active_library_root()?;
        let mut journal = self.journal_for(&library_root, operation_id)?;
        if journal.phase != SourcePromotionPhase::Finalized {
            return Err(SourcePromotionError::Validation(
                "the Source Promotion is not ready to finalize".into(),
            ));
        }
        journal.phase = SourcePromotionPhase::Finalizing;
        self.write_journal(&library_root, &journal)?;
        if let Err(error) = self.complete_finalization(&library_root, &journal) {
            self.write_gate.mark_blocked();
            return Err(SourcePromotionError::RecoveryRequired(format!(
                "complete Source Promotion finalization: {error}"
            )));
        }
        Ok(())
    }

    fn complete_finalization(
        &self,
        library_root: &Path,
        journal: &SourcePromotionJournal,
    ) -> Result<(), SourcePromotionError> {
        for replacement in &journal.mutations.replacements {
            self.filesystem
                .commit_replaced_skill(replacement, library_root)?;
        }
        for backup in &journal.mutations.removed_backups {
            self.filesystem.discard_library_entity_backup(
                &backup.backup_path,
                library_root,
                Some(&backup.fingerprint),
            )?;
        }
        if self
            .filesystem
            .path_is_occupied(&journal.staging_operation_root)?
        {
            self.discard_promotion_staging(library_root, journal)?;
        }
        self.filesystem
            .finish_source_promotion_journal(library_root, &journal.operation_id)?;
        Ok(())
    }

    fn complete_undo(
        &self,
        library_root: &Path,
        journal: &mut SourcePromotionJournal,
    ) -> Result<u64, SourcePromotionError> {
        let record = journal.record.as_ref().ok_or_else(|| {
            SourcePromotionError::RecoveryRequired(
                "the Source Promotion undo journal is missing its target release".into(),
            )
        })?;
        let snapshot_version = if self.store.source_promotion_is_committed(record)? {
            self.store.undo_source_promotion(record, &journal.legacy)?
        } else {
            0
        };
        self.rollback_filesystem_changes(library_root, &mut journal.mutations)?;
        self.restore_legacy_manifest(
            &library_root.join("remotes"),
            &journal.legacy.remote_id,
            journal.previous_manifest.as_ref(),
        )?;
        self.discard_promotion_staging(library_root, journal)?;
        self.filesystem
            .finish_source_promotion_journal(library_root, &journal.operation_id)?;
        Ok(snapshot_version)
    }

    fn write_journal(
        &self,
        library_root: &Path,
        journal: &SourcePromotionJournal,
    ) -> Result<(), SourcePromotionError> {
        let bytes = serde_json::to_vec_pretty(journal).map_err(|error| {
            SourcePromotionError::RecoveryRequired(format!(
                "serialize the Source Promotion journal: {error}"
            ))
        })?;
        self.filesystem.write_source_promotion_journal(
            library_root,
            &journal.operation_id,
            &bytes,
        )?;
        Ok(())
    }

    /// Once a physical/SQLite mutation has completed, failure to durably
    /// advance the cursor is itself a recovery condition.  The previous
    /// cursor deliberately remains available for startup convergence, while
    /// ordinary product writes stay closed in this process.
    fn advance_after_mutation(
        &self,
        library_root: &Path,
        journal: &mut SourcePromotionJournal,
        phase: SourcePromotionPhase,
        action: &'static str,
    ) -> Result<(), SourcePromotionError> {
        journal.phase = phase;
        if let Err(error) = self.write_journal(library_root, journal) {
            self.write_gate.mark_blocked();
            return Err(SourcePromotionError::RecoveryRequired(format!(
                "{action}: {error}"
            )));
        }
        Ok(())
    }

    fn decode_journal(
        &self,
        operation_id: &str,
        bytes: &[u8],
    ) -> Result<SourcePromotionJournal, SourcePromotionError> {
        let journal: SourcePromotionJournal = serde_json::from_slice(bytes).map_err(|error| {
            SourcePromotionError::RecoveryRequired(format!(
                "parse Source Promotion journal '{operation_id}': {error}"
            ))
        })?;
        if journal.version != SOURCE_PROMOTION_JOURNAL_VERSION
            || journal.operation_id != operation_id
            || !self.accepts_operation_id(operation_id)
        {
            return Err(SourcePromotionError::RecoveryRequired(format!(
                "Source Promotion journal '{operation_id}' has an unsupported identity or version"
            )));
        }
        Ok(journal)
    }

    fn accepts_operation_id(&self, operation_id: &str) -> bool {
        match self.mode {
            SourcePromotionMode::LegacyPromotion => operation_id.starts_with("source-promotion-"),
            SourcePromotionMode::SourceUpdate => operation_id.starts_with("source-update-"),
        }
    }

    fn journal_for(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<SourcePromotionJournal, SourcePromotionError> {
        self.filesystem
            .list_source_promotion_journals(library_root)?
            .into_iter()
            .find(|(candidate, _)| candidate == operation_id)
            .ok_or_else(|| {
                SourcePromotionError::Validation(
                    "the Source Promotion result window is no longer available".into(),
                )
            })
            .and_then(|(candidate, bytes)| self.decode_journal(&candidate, &bytes))
    }

    fn discard_promotion_staging(
        &self,
        library_root: &Path,
        journal: &SourcePromotionJournal,
    ) -> Result<(), SourcePromotionError> {
        self.filesystem.discard_staging(
            &journal.staging_operation_root,
            library_root,
            journal.staging_fingerprint.as_ref(),
        )?;
        Ok(())
    }

    fn finish_rolled_back_promotion(
        &self,
        library_root: &Path,
        journal: &SourcePromotionJournal,
    ) -> Result<(), SourcePromotionError> {
        if self
            .filesystem
            .path_is_occupied(&journal.staging_operation_root)?
        {
            self.discard_promotion_staging(library_root, journal)?;
        }
        self.filesystem
            .finish_source_promotion_journal(library_root, &journal.operation_id)?;
        Ok(())
    }

    /// Catalog truth alone is never enough to reopen writes after recovery.
    /// Every frozen member, retained Local Link and activation must still
    /// match the exact source-level outcome recorded before confirmation.
    fn verify_final_source(
        &self,
        library_root: &Path,
        journal: &SourcePromotionJournal,
    ) -> Result<(), SourcePromotionError> {
        let record = journal.record.as_ref().ok_or_else(|| {
            SourcePromotionError::RecoveryRequired(
                "the Source Promotion journal is missing its target release".into(),
            )
        })?;
        let target_manifest = journal.target_manifest.as_ref().ok_or_else(|| {
            SourcePromotionError::RecoveryRequired(
                "the Source Promotion journal is missing its target manifest".into(),
            )
        })?;
        let manifest = self
            .filesystem
            .read_remote_parent_manifest(&library_root.join("remotes"), &record.remote_id)?;
        if manifest.as_ref() != Some(target_manifest) {
            return Err(SourcePromotionError::RecoveryRequired(
                "the Source Promotion manifest no longer matches its frozen release".into(),
            ));
        }
        for member in &record.members {
            if !self
                .filesystem
                .path_is_directory(&member.final_entity_path)?
                || self.filesystem.tree_hash(&member.final_entity_path)?
                    != member.current_baseline_hash
            {
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "the Source Promotion member '{}' no longer matches its frozen release",
                    member.directory_name
                )));
            }
        }
        for backup in &journal.mutations.removed_backups {
            if self.filesystem.path_is_occupied(&backup.source_path)? {
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "removed Source Promotion member '{}' returned to the Library",
                    backup.source_path.display()
                )));
            }
        }
        for moved in &journal.mutations.moved_links {
            let target_snapshot = self.filesystem.staged_tree_snapshot(&moved.target_path)?;
            if self.filesystem.path_is_occupied(&moved.source_path)?
                || !same_directory_identity(&target_snapshot.root, &moved.expected_snapshot.root)
                || target_snapshot.content_hash != moved.expected_snapshot.content_hash
            {
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "Local Link target '{}' no longer matches the frozen Source Promotion",
                    moved.target_path.display()
                )));
            }
        }
        for activation in &journal.mutations.activations {
            let expected = match &activation.new_target {
                Some(target) => ActivationEntrySnapshot::Symlink {
                    target: target.clone(),
                },
                None => ActivationEntrySnapshot::Missing,
            };
            if self
                .filesystem
                .activation_snapshot(&activation.entry_path)?
                != expected
            {
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "Source Promotion Activation '{}' no longer matches the frozen release",
                    activation.entry_path.display()
                )));
            }
        }
        Ok(())
    }

    fn prepare_target_members(
        &self,
        library_root: &Path,
        operation_id: &str,
        confirmed: &ConfirmedSourcePromotion,
        legacy_record: &LegacySourcePromotionRecord,
    ) -> Result<PreparedPromotion, SourcePromotionError> {
        let draft = &confirmed.draft;
        let lock_reports = self
            .lock_store
            .discover()
            .map_err(|error| SourcePromotionError::Validation(error.to_string()))?;
        let target_directory_names = draft
            .target_members
            .iter()
            .map(|member| member.member.directory_name.clone())
            .collect::<Vec<_>>();
        if self.mode == SourcePromotionMode::SourceUpdate
            && self.source_update_external_reappeared(
                legacy_record,
                &lock_reports,
                &target_directory_names,
            )?
        {
            return Err(SourcePromotionError::OwnershipConflict);
        }
        let installer_roots = lock_reports
            .into_iter()
            .filter_map(|report| report.path.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        let legacy_by_id = draft
            .existing_members
            .iter()
            .map(|member| (member.member.skill_id.0.as_str(), member))
            .collect::<BTreeMap<_, _>>();
        let mut target_to_legacy = draft
            .target_members
            .iter()
            .filter_map(|target| {
                target
                    .legacy_skill_id
                    .as_ref()
                    .map(|id| (target.member.skill_path.clone(), id.0.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        for (skill_id, resolution) in &confirmed.resolutions {
            if let Some(UpstreamMemberRemovedResolution::ExplicitMemberMapping {
                target_skill_path,
            }) = resolution.removed.as_ref()
            {
                target_to_legacy.insert(target_skill_path.clone(), skill_id.clone());
            }
        }

        let temporary_mirror_root = tempfile::Builder::new()
            .prefix("skill-man-source-promotion-")
            .tempdir()
            .map_err(|source| {
                SourcePromotionError::Source(SourceError::Io {
                    operation: "create temporary Git Source Promotion mirror",
                    path: std::env::temp_dir(),
                    source,
                })
            })?;
        let mut spec = parse_git_source_input(&draft.canonical_url)
            .map_err(|error| SourcePromotionError::Validation(error.to_string()))?;
        spec.requested_ref = Some(draft.tracking_ref.clone());
        let mirror = git_mirror_path(temporary_mirror_root.path(), &spec.url);
        let report = self.git_source.fetch_mirror(&spec.url, &mirror)?;
        let resolved = resolve_git_ref(self.git_source.as_ref(), &mirror, &spec, &report)
            .map_err(|error| SourcePromotionError::Validation(error.to_string()))?;
        if resolved.commit != draft.resolved_commit {
            return Err(SourcePromotionError::Validation(
                "the Source Group Draft is stale; the discovered release changed".into(),
            ));
        }

        let mut members = Vec::with_capacity(draft.target_members.len());
        for (index, target) in draft.target_members.iter().enumerate() {
            let staging_path = library_root
                .join("staging")
                .join(operation_id)
                .join(format!("target-{index}"));
            self.git_source.stage_skill(
                &mirror,
                &draft.resolved_commit,
                &target.member.skill_path,
                &staging_path,
            )?;
            let staged_snapshot = self.filesystem.staged_tree_snapshot(&staging_path)?;
            crate::core::import::validate_staged_tree(self.filesystem.as_ref(), &staged_snapshot)
                .map_err(|error| SourcePromotionError::Validation(error.to_string()))?;
            let document = self.filesystem.read_skill_document(&staging_path)?;
            let metadata = parse_skill_metadata(&document);
            let remote_id = target_to_legacy.get(&target.member.skill_path);
            let (record, replace_existing) = if let Some(legacy_id) = remote_id {
                let existing = legacy_by_id.get(legacy_id.as_str()).ok_or_else(|| {
                    SourcePromotionError::Validation(
                        "an Explicit Member Mapping does not belong to this Source Group Draft"
                            .into(),
                    )
                })?;
                let current_snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&legacy_final_path(legacy_record, legacy_id)?)?;
                if current_snapshot.content_hash != existing.member.current_tree_hash {
                    return Err(SourcePromotionError::Validation(format!(
                        "Legacy member '{}' changed after the Source Group Draft",
                        existing.member.directory_name
                    )));
                }
                let resolution = confirmed.resolutions.get(legacy_id);
                let keep_modified = match existing.state {
                    SourcePromotionMemberState::UpdateToTarget => false,
                    SourcePromotionMemberState::ModifiedMemberResolutionRequired => {
                        matches!(
                            resolution.and_then(|value| value.modified),
                            Some(ModifiedMemberResolution::KeepModified)
                        )
                    }
                    SourcePromotionMemberState::UpstreamMemberRemoved => matches!(
                        resolution.and_then(|value| value.modified),
                        Some(ModifiedMemberResolution::KeepModified)
                    ),
                };
                let current_baseline_hash = if keep_modified {
                    current_snapshot.content_hash.clone()
                } else {
                    staged_snapshot.content_hash.clone()
                };
                (
                    SourcePromotionMemberRecord {
                        origin: SourcePromotionMemberOrigin::Legacy,
                        skill_id: existing.member.skill_id.clone(),
                        directory_name: existing.member.directory_name.clone(),
                        identity_key: skill_identity_key(&existing.member.directory_name),
                        display_name: metadata
                            .name
                            .unwrap_or_else(|| existing.member.directory_name.clone()),
                        description: metadata.description.unwrap_or_default(),
                        final_entity_path: legacy_final_path(legacy_record, legacy_id)?,
                        skill_path: target.member.skill_path.clone(),
                        remote_baseline_hash: staged_snapshot.content_hash.clone(),
                        current_baseline_hash,
                        health: if keep_modified {
                            Health::Modified
                        } else {
                            Health::Healthy
                        },
                    },
                    (!keep_modified).then_some(current_snapshot),
                )
            } else {
                let final_entity_path = library_root
                    .join("skills")
                    .join(&target.member.directory_name);
                if self.filesystem.path_is_occupied(&final_entity_path)? {
                    return Err(SourcePromotionError::Validation(format!(
                        "new Source Member '{}' conflicts with an occupied Library path",
                        target.member.directory_name
                    )));
                }
                (
                    SourcePromotionMemberRecord {
                        origin: SourcePromotionMemberOrigin::New,
                        skill_id: SkillId(self.next_uuid()),
                        directory_name: target.member.directory_name.clone(),
                        identity_key: skill_identity_key(&target.member.directory_name),
                        display_name: metadata
                            .name
                            .unwrap_or_else(|| target.member.directory_name.clone()),
                        description: metadata.description.unwrap_or_default(),
                        final_entity_path,
                        skill_path: target.member.skill_path.clone(),
                        remote_baseline_hash: staged_snapshot.content_hash.clone(),
                        current_baseline_hash: staged_snapshot.content_hash.clone(),
                        health: Health::Healthy,
                    },
                    None,
                )
            };
            members.push(PreparedPromotionMember {
                record,
                staged_path: staging_path,
                staged_snapshot,
                replace_existing,
            });
        }

        let mut removed_records = Vec::new();
        let mut removed_paths = Vec::new();
        for existing in &draft.existing_members {
            if existing.state != SourcePromotionMemberState::UpstreamMemberRemoved {
                continue;
            }
            let Some(resolution) = confirmed.resolutions.get(&existing.member.skill_id.0) else {
                continue;
            };
            match resolution.removed.as_ref() {
                Some(UpstreamMemberRemovedResolution::ExplicitMemberMapping { .. }) => {}
                Some(UpstreamMemberRemovedResolution::Remove) => {
                    let source_path =
                        legacy_final_path(legacy_record, &existing.member.skill_id.0)?;
                    let source_parent = source_path.parent().ok_or_else(|| {
                        SourcePromotionError::Validation(
                            "Legacy member has no Library parent directory".into(),
                        )
                    })?;
                    let source_parent_fingerprint =
                        self.filesystem.directory_fingerprint(source_parent)?;
                    removed_records.push(SourcePromotionRemovedMemberRecord::Remove {
                        skill_id: existing.member.skill_id.clone(),
                    });
                    removed_paths.push(PreparedRemovedMember {
                        skill_id: existing.member.skill_id.clone(),
                        source_path,
                        local_link_target: None,
                        source_parent: source_parent_fingerprint,
                        local_link_target_parent: None,
                        expected_tree_hash: existing.member.current_tree_hash.clone(),
                    });
                }
                Some(UpstreamMemberRemovedResolution::LocalLink { target_directory }) => {
                    let raw_target = PathBuf::from(target_directory);
                    // Persist the resolved existing ancestor, not the raw
                    // spelling.  macOS aliases `/tmp` and `/var`; retaining
                    // the raw spelling would make a successful move look
                    // unlike its durable fingerprint during recovery.
                    let target = self.filesystem.normalize_configured_path(&raw_target)?;
                    let target_parent = target.parent().ok_or_else(|| {
                        SourcePromotionError::Validation(
                            "Local Link target has no external parent directory".into(),
                        )
                    })?;
                    if !target.is_absolute()
                        || !self.filesystem.path_is_directory(target_parent)?
                        || target.starts_with(library_root)
                        || legacy_record
                            .forbidden_local_link_roots
                            .iter()
                            .chain(installer_roots.iter())
                            .any(|root| target.starts_with(root))
                        || self.filesystem.path_is_occupied(&target)?
                        || !self.filesystem.path_has_no_symlink_component(&target)?
                    {
                        return Err(SourcePromotionError::Validation(format!(
                            "Local Link target for '{}' is not a safe unoccupied external path",
                            existing.member.directory_name
                        )));
                    }
                    let source_path =
                        legacy_final_path(legacy_record, &existing.member.skill_id.0)?;
                    let source_parent = source_path.parent().ok_or_else(|| {
                        SourcePromotionError::Validation(
                            "Legacy member has no Library parent directory".into(),
                        )
                    })?;
                    let source_parent_fingerprint =
                        self.filesystem.directory_fingerprint(source_parent)?;
                    let target_parent_fingerprint =
                        self.filesystem.directory_fingerprint(target_parent)?;
                    removed_records.push(SourcePromotionRemovedMemberRecord::LocalLink {
                        skill_id: existing.member.skill_id.clone(),
                        final_entity_path: target.clone(),
                    });
                    removed_paths.push(PreparedRemovedMember {
                        skill_id: existing.member.skill_id.clone(),
                        source_path,
                        local_link_target: Some(target),
                        source_parent: source_parent_fingerprint,
                        local_link_target_parent: Some(target_parent_fingerprint),
                        expected_tree_hash: existing.member.current_tree_hash.clone(),
                    });
                }
                None => unreachable!("draft confirmation requires a removed-member resolution"),
            }
        }
        Ok(PreparedPromotion {
            members,
            removed_records,
            removed_paths,
        })
    }

    fn apply_filesystem_changes(
        &self,
        library_root: &Path,
        operation_id: &str,
        prepared: &PreparedPromotion,
        legacy_record: &LegacySourcePromotionRecord,
        journal: &mut SourcePromotionJournal,
    ) -> Result<(), SourcePromotionError> {
        for member in &prepared.members {
            let Some(existing_snapshot) = &member.replace_existing else {
                continue;
            };
            let normalized_library_root =
                self.filesystem.normalize_configured_path(library_root)?;
            let normalized_final_entity_path = self
                .filesystem
                .normalize_configured_path(&member.record.final_entity_path)?;
            let directory_name = member.record.final_entity_path.file_name().ok_or_else(|| {
                SourcePromotionError::Validation(
                    "a Source Promotion member has no directory name".into(),
                )
            })?;
            let backup_path = normalized_library_root
                .join("operations")
                .join(operation_id)
                .join("backup")
                .join(directory_name);
            let planned = FileReplacement {
                final_entity_path: normalized_final_entity_path.clone(),
                installed_fingerprint: moved_fingerprint(
                    &member.staged_snapshot.root,
                    &normalized_final_entity_path,
                ),
                backup_path: backup_path.clone(),
                backup_fingerprint: moved_fingerprint(&existing_snapshot.root, &backup_path),
                original_tree_snapshot: existing_snapshot.clone(),
            };
            journal.mutations.replacements.push(planned.clone());
            self.write_journal(library_root, journal)?;
            let replacement = self.filesystem.replace_staged_skill(
                &member.staged_path,
                &member.record.final_entity_path,
                library_root,
                operation_id,
                &member.staged_snapshot,
                existing_snapshot,
            )?;
            if !same_replacement(&replacement, &planned) {
                return Err(SourcePromotionError::RecoveryRequired(
                    "the replacement result no longer matches its frozen Source Promotion action"
                        .into(),
                ));
            }
        }
        for removed in &prepared.removed_paths {
            let legacy_member = legacy_member_record(legacy_record, &removed.skill_id)?;
            for activation in &legacy_member.activations {
                if !activation.desired_enabled {
                    continue;
                }
                if activation.target_path != removed.source_path
                    || self
                        .filesystem
                        .activation_snapshot(&activation.entry_path)?
                        != (ActivationEntrySnapshot::Symlink {
                            target: activation.target_path.clone(),
                        })
                {
                    return Err(SourcePromotionError::Validation(format!(
                        "Activation for Legacy member '{}' changed before Source Promotion",
                        legacy_member.directory_name
                    )));
                }
                journal
                    .mutations
                    .activations
                    .push(PromotionActivationMutation {
                        entry_path: activation.entry_path.clone(),
                        old_target: activation.target_path.clone(),
                        new_target: removed.local_link_target.clone(),
                    });
                self.write_journal(library_root, journal)?;
                self.filesystem.remove_activation(&activation.entry_path)?;
            }
        }
        for (index, removed) in prepared.removed_paths.iter().enumerate() {
            let snapshot = self.filesystem.staged_tree_snapshot(&removed.source_path)?;
            if snapshot.content_hash != removed.expected_tree_hash {
                return Err(SourcePromotionError::Validation(format!(
                    "Legacy member '{}' changed after the Source Group Draft",
                    removed.source_path.display()
                )));
            }
            if let Some(target) = &removed.local_link_target {
                let target_parent = removed.local_link_target_parent.as_ref().ok_or_else(|| {
                    SourcePromotionError::RecoveryRequired(
                        "the frozen Local Link action is missing its target parent identity".into(),
                    )
                })?;
                journal.mutations.moved_links.push(MovedLibraryLink {
                    source_path: removed.source_path.clone(),
                    target_path: target.clone(),
                    expected_snapshot: StagedTreeSnapshot {
                        root: moved_fingerprint(&snapshot.root, target),
                        ..snapshot.clone()
                    },
                    source_parent: removed.source_parent.clone(),
                    target_parent: target_parent.clone(),
                });
                self.write_journal(library_root, journal)?;
                let moved = self.filesystem.move_directory_nofollow(
                    &removed.source_path,
                    target,
                    &snapshot,
                    &removed.source_parent,
                    target_parent,
                )?;
                let planned = journal
                    .mutations
                    .moved_links
                    .last()
                    .expect("Source Promotion Local Link was just journaled");
                if !same_directory_identity(&moved, &planned.expected_snapshot.root) {
                    return Err(SourcePromotionError::RecoveryRequired(
                        "the Local Link move no longer matches its frozen Source Promotion action"
                            .into(),
                    ));
                }
            } else {
                let backup_path = library_root
                    .join("operations")
                    .join(operation_id)
                    .join("promotion-removed")
                    .join(index.to_string());
                let fingerprint = moved_fingerprint(&snapshot.root, &backup_path);
                let planned = RemovedLibraryBackup {
                    source_path: removed.source_path.clone(),
                    backup_path,
                    fingerprint,
                };
                journal.mutations.removed_backups.push(planned.clone());
                self.write_journal(library_root, journal)?;
                let fingerprint = self.filesystem.backup_library_entity(
                    &removed.source_path,
                    &planned.backup_path,
                    library_root,
                )?;
                if !same_directory_identity(&fingerprint, &planned.fingerprint) {
                    return Err(SourcePromotionError::RecoveryRequired(
                        "the removed member backup no longer matches its frozen Source Promotion action"
                            .into(),
                    ));
                }
            }
        }
        for activation in &journal.mutations.activations {
            if let Some(target) = &activation.new_target {
                self.filesystem
                    .create_activation(target, &activation.entry_path)?;
            }
        }
        for member in &prepared.members {
            if member.record.origin != SourcePromotionMemberOrigin::New {
                continue;
            }
            let planned_fingerprint = moved_fingerprint(
                &member.staged_snapshot.root,
                &member.record.final_entity_path,
            );
            journal.mutations.installed_new.push((
                member.record.final_entity_path.clone(),
                planned_fingerprint.clone(),
            ));
            self.write_journal(library_root, journal)?;
            let fingerprint = self.filesystem.install_staged_skill(
                &member.staged_path,
                &member.record.final_entity_path,
                library_root,
                operation_id,
                &member.staged_snapshot,
            )?;
            if !same_directory_identity(&fingerprint, &planned_fingerprint) {
                return Err(SourcePromotionError::RecoveryRequired(
                    "the new member install no longer matches its frozen Source Promotion action"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    fn rollback_filesystem_changes(
        &self,
        library_root: &Path,
        mutations: &mut PromotionMutations,
    ) -> Result<(), SourcePromotionError> {
        for (path, fingerprint) in mutations.installed_new.iter().rev() {
            if self.filesystem.path_is_occupied(path)? {
                self.filesystem
                    .discard_installed_skill(path, library_root, fingerprint)?;
            }
        }
        for backup in mutations.removed_backups.iter().rev() {
            if self.filesystem.path_is_occupied(&backup.source_path)? {
                let restored = self.filesystem.staged_tree_snapshot(&backup.source_path)?;
                if !same_directory_identity(&restored.root, &backup.fingerprint) {
                    return Err(SourcePromotionError::RecoveryRequired(format!(
                        "the restored Legacy member '{}' changed during recovery",
                        backup.source_path.display()
                    )));
                }
            } else {
                self.filesystem.restore_library_entity(
                    &backup.backup_path,
                    &backup.source_path,
                    library_root,
                    &backup.fingerprint,
                )?;
            }
        }
        for moved in mutations.moved_links.iter().rev() {
            if self.filesystem.path_is_occupied(&moved.source_path)? {
                let restored = self.filesystem.staged_tree_snapshot(&moved.source_path)?;
                if !same_directory_identity(&restored.root, &moved.expected_snapshot.root)
                    || restored.content_hash != moved.expected_snapshot.content_hash
                {
                    return Err(SourcePromotionError::RecoveryRequired(format!(
                        "the restored Local Link member '{}' changed during recovery",
                        moved.source_path.display()
                    )));
                }
                continue;
            }
            let moved_snapshot = self.filesystem.staged_tree_snapshot(&moved.target_path)?;
            if !same_directory_identity(&moved_snapshot.root, &moved.expected_snapshot.root)
                || moved_snapshot.content_hash != moved.expected_snapshot.content_hash
            {
                return Err(SourcePromotionError::RecoveryRequired(format!(
                    "the Local Link target '{}' changed before rollback",
                    moved.target_path.display()
                )));
            }
            self.filesystem.move_directory_nofollow(
                &moved.target_path,
                &moved.source_path,
                &moved.expected_snapshot,
                &moved.target_parent,
                &moved.source_parent,
            )?;
        }
        for replacement in mutations.replacements.iter().rev() {
            self.filesystem
                .rollback_replaced_skill(replacement, library_root)?;
        }
        for activation in mutations.activations.iter().rev() {
            if let Some(new_target) = &activation.new_target
                && matches!(
                    self.filesystem.activation_snapshot(&activation.entry_path)?,
                    ActivationEntrySnapshot::Symlink { target } if target == *new_target
                )
            {
                self.filesystem.remove_activation(&activation.entry_path)?;
            }
            if matches!(
                self.filesystem
                    .activation_snapshot(&activation.entry_path)?,
                ActivationEntrySnapshot::Missing
            ) {
                self.filesystem
                    .create_activation(&activation.old_target, &activation.entry_path)?;
            }
        }
        Ok(())
    }

    fn restore_legacy_manifest(
        &self,
        remotes_root: &Path,
        remote_id: &str,
        previous_manifest: Option<&RemoteParentManifest>,
    ) -> Result<(), FileSystemError> {
        match previous_manifest {
            Some(manifest) => self
                .filesystem
                .write_remote_parent_manifest(remotes_root, manifest),
            None => self
                .filesystem
                .remove_remote_parent_manifest(remotes_root, remote_id),
        }
    }

    fn active_library_root(&self) -> Result<PathBuf, SourcePromotionError> {
        match &self.home_context {
            Some(context) => context
                .bound_home()
                .map(|home| home.path)
                .map_err(|error| SourcePromotionError::Validation(error.to_string())),
            None => Ok(self.configured_library_root.clone()),
        }
    }

    fn ensure_writes_ready(&self) -> Result<(), SourcePromotionError> {
        if self.write_gate.is_product_write_open() {
            Ok(())
        } else {
            Err(SourcePromotionError::Validation(
                "Source Promotion is unavailable while recovery is in progress".into(),
            ))
        }
    }

    fn next_operation_id(&self) -> String {
        let number = self.next_id.fetch_add(1, Ordering::Relaxed);
        let prefix = match self.mode {
            SourcePromotionMode::LegacyPromotion => "source-promotion",
            SourcePromotionMode::SourceUpdate => "source-update",
        };
        format!("{prefix}-{}-{number}", self.clock.unix_epoch_nanos())
    }

    /// A managed source has no external owner. Reappearance is therefore a
    /// conflict, whether an installer re-declares its source in a lock or
    /// recreates one of its entity paths without a lock. We repeat this
    /// check immediately before staging so a preview cannot race it.
    fn source_update_external_reappeared(
        &self,
        record: &LegacySourcePromotionRecord,
        lock_reports: &[LockFileReport],
        target_directory_names: &[String],
    ) -> Result<bool, SourcePromotionError> {
        let source_url = parse_git_source_input(&record.canonical_url)
            .map_err(|error| SourcePromotionError::Validation(error.to_string()))?
            .url;
        let member_names = record
            .members
            .iter()
            .map(|member| member.directory_name.as_str())
            .chain(target_directory_names.iter().map(String::as_str))
            .collect::<BTreeSet<_>>();
        for report in lock_reports {
            if report.fault.is_some() {
                return Ok(true);
            }
            if report
                .entry_faults
                .iter()
                .any(|fault| member_names.contains(fault.name.as_str()))
            {
                return Ok(true);
            }
            for entry in &report.entries {
                let declares_member = member_names.contains(entry.name.as_str());
                let declares_source = parse_git_source_input(&entry.source_url)
                    .ok()
                    .is_some_and(|entry_source| entry_source.url == source_url);
                if declares_member || declares_source {
                    return Ok(true);
                }
            }
        }

        let mut roots = self
            .external_owner_roots
            .iter()
            .chain(record.forbidden_local_link_roots.iter())
            .cloned()
            .collect::<BTreeSet<_>>();
        for report in lock_reports {
            if let Some(parent) = report.path.parent() {
                roots.insert(parent.to_path_buf());
                roots.insert(parent.join("skills"));
            }
        }
        for root in roots {
            for member_name in &member_names {
                if self.filesystem.path_is_occupied(&root.join(member_name))? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn next_uuid(&self) -> String {
        let number = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut bytes = [0_u8; 16];
        if self.filesystem.read_entropy(&mut bytes).is_err() {
            bytes[..8].copy_from_slice(&(self.clock.unix_epoch_nanos() as u64).to_be_bytes());
            bytes[8..].copy_from_slice(&number.to_be_bytes());
        }
        crate::seams::clock::uuid_v4_shape(&mut bytes)
    }
}

#[derive(Clone, Debug)]
struct PreparedPromotionMember {
    record: SourcePromotionMemberRecord,
    staged_path: PathBuf,
    staged_snapshot: StagedTreeSnapshot,
    replace_existing: Option<StagedTreeSnapshot>,
}

#[derive(Clone, Debug)]
struct PreparedPromotion {
    members: Vec<PreparedPromotionMember>,
    removed_records: Vec<SourcePromotionRemovedMemberRecord>,
    removed_paths: Vec<PreparedRemovedMember>,
}

#[derive(Clone, Debug)]
struct PreparedRemovedMember {
    skill_id: SkillId,
    source_path: PathBuf,
    local_link_target: Option<PathBuf>,
    source_parent: DirectoryFingerprint,
    local_link_target_parent: Option<DirectoryFingerprint>,
    expected_tree_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RemovedLibraryBackup {
    source_path: PathBuf,
    backup_path: PathBuf,
    fingerprint: DirectoryFingerprint,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct PromotionMutations {
    replacements: Vec<FileReplacement>,
    removed_backups: Vec<RemovedLibraryBackup>,
    moved_links: Vec<MovedLibraryLink>,
    installed_new: Vec<(PathBuf, DirectoryFingerprint)>,
    activations: Vec<PromotionActivationMutation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PromotionActivationMutation {
    entry_path: PathBuf,
    old_target: PathBuf,
    new_target: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MovedLibraryLink {
    source_path: PathBuf,
    target_path: PathBuf,
    expected_snapshot: StagedTreeSnapshot,
    source_parent: DirectoryFingerprint,
    target_parent: DirectoryFingerprint,
}

fn moved_fingerprint(
    fingerprint: &DirectoryFingerprint,
    canonical_path: &Path,
) -> DirectoryFingerprint {
    DirectoryFingerprint {
        canonical_path: canonical_path.to_path_buf(),
        device: fingerprint.device,
        inode: fingerprint.inode,
    }
}

fn same_directory_identity(left: &DirectoryFingerprint, right: &DirectoryFingerprint) -> bool {
    left.device == right.device && left.inode == right.inode
}

fn same_replacement(left: &FileReplacement, right: &FileReplacement) -> bool {
    same_directory_identity(&left.installed_fingerprint, &right.installed_fingerprint)
        && same_directory_identity(&left.backup_fingerprint, &right.backup_fingerprint)
        && same_directory_identity(
            &left.original_tree_snapshot.root,
            &right.original_tree_snapshot.root,
        )
        && left.original_tree_snapshot.content_hash == right.original_tree_snapshot.content_hash
}

fn legacy_final_path(
    legacy: &LegacySourcePromotionRecord,
    skill_id: &str,
) -> Result<PathBuf, SourcePromotionError> {
    legacy
        .members
        .iter()
        .find(|member| member.skill_id.0 == skill_id)
        .map(|member| member.final_entity_path.clone())
        .ok_or_else(|| {
            SourcePromotionError::Validation(
                "the selected Legacy Source Promotion member disappeared".into(),
            )
        })
}

fn legacy_member_record<'a>(
    legacy: &'a LegacySourcePromotionRecord,
    skill_id: &SkillId,
) -> Result<
    &'a crate::seams::source_promotion_store::LegacySourcePromotionMemberRecord,
    SourcePromotionError,
> {
    legacy
        .members
        .iter()
        .find(|member| member.skill_id == *skill_id)
        .ok_or_else(|| {
            SourcePromotionError::Validation(
                "the selected Legacy Source Promotion member disappeared".into(),
            )
        })
}

fn source_type_for(canonical_url: &str) -> &'static str {
    if canonical_url.starts_with("https://github.com/") {
        "github"
    } else if canonical_url.starts_with("https://gitlab.com/") {
        "gitlab"
    } else {
        "git"
    }
}

impl SourcePromotionDraft {
    /// Classify an explicitly selected legacy parent against one complete,
    /// fresh Source Release.  This deliberately makes no decisions and no
    /// writes; a caller may discard the draft at any time.
    pub fn build(
        legacy: LegacySourcePromotion,
        preview: SourceGroupPreview,
    ) -> Result<Self, SourcePromotionDraftError> {
        validate_legacy(&legacy)?;
        if preview.source_url != legacy.canonical_url {
            return Err(SourcePromotionDraftError::RepositoryMismatch);
        }
        if preview.tracking_ref != legacy.tracking_ref {
            return Err(SourcePromotionDraftError::TrackingRefMismatch);
        }
        let target_paths = target_by_path(&preview.members)?
            .into_keys()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let legacy_by_path = legacy
            .members
            .iter()
            .map(|member| (member.skill_path.clone(), member.skill_id.clone()))
            .collect::<BTreeMap<_, _>>();

        let mut target_members = preview
            .members
            .into_iter()
            .map(|member| SourcePromotionTargetMemberDraft {
                legacy_skill_id: legacy_by_path.get(&member.skill_path).cloned(),
                member,
            })
            .collect::<Vec<_>>();
        target_members.sort_by(|left, right| left.member.skill_path.cmp(&right.member.skill_path));

        let mut existing_members = legacy
            .members
            .into_iter()
            .map(|member| {
                let state = if target_paths.contains(&member.skill_path) {
                    if member.current_tree_hash == member.current_baseline_hash {
                        SourcePromotionMemberState::UpdateToTarget
                    } else {
                        SourcePromotionMemberState::ModifiedMemberResolutionRequired
                    }
                } else {
                    SourcePromotionMemberState::UpstreamMemberRemoved
                };
                SourcePromotionExistingMemberDraft { member, state }
            })
            .collect::<Vec<_>>();
        existing_members
            .sort_by(|left, right| left.member.skill_path.cmp(&right.member.skill_path));

        Ok(Self {
            remote_id: legacy.remote_id,
            provider: preview.provider,
            canonical_url: preview.source_url,
            tracking_ref: preview.tracking_ref,
            resolved_commit: preview.resolved_commit,
            existing_members,
            target_members,
        })
    }

    /// Validate that every conflict was consciously resolved and that an
    /// Explicit Member Mapping points to a target path which is neither
    /// occupied by an exact legacy match nor reused by another mapping.
    pub fn confirm(
        self,
        resolutions: Vec<SourcePromotionResolution>,
    ) -> Result<ConfirmedSourcePromotion, SourcePromotionDraftError> {
        let mut supplied = BTreeMap::new();
        for resolution in resolutions {
            let key = resolution.skill_id.0.clone();
            if supplied.insert(key.clone(), resolution).is_some() {
                return Err(SourcePromotionDraftError::InvalidResolution(key));
            }
        }

        let existing_ids = self
            .existing_members
            .iter()
            .map(|existing| existing.member.skill_id.0.as_str())
            .collect::<BTreeSet<_>>();
        let exact_target_paths = self
            .target_members
            .iter()
            .filter_map(|target| {
                target
                    .legacy_skill_id
                    .as_ref()
                    .map(|legacy_id| (target.member.skill_path.as_str(), legacy_id))
            })
            .collect::<BTreeMap<_, _>>();
        let target_paths = self
            .target_members
            .iter()
            .map(|target| target.member.skill_path.as_str())
            .collect::<BTreeSet<_>>();
        let mut claimed_mapping_paths = BTreeSet::new();

        for existing in &self.existing_members {
            let required = matches!(
                existing.state,
                SourcePromotionMemberState::ModifiedMemberResolutionRequired
                    | SourcePromotionMemberState::UpstreamMemberRemoved
            );
            let resolution = supplied.get(&existing.member.skill_id.0);
            if required && resolution.is_none() {
                return Err(SourcePromotionDraftError::MissingResolution(
                    existing.member.directory_name.clone(),
                ));
            }
            let Some(resolution) = resolution else {
                continue;
            };
            if resolution.skill_id != existing.member.skill_id {
                return Err(SourcePromotionDraftError::MismatchedResolution(
                    resolution.skill_id.0.clone(),
                ));
            }
            match existing.state {
                SourcePromotionMemberState::UpdateToTarget => {
                    return Err(SourcePromotionDraftError::UnexpectedResolution(
                        existing.member.directory_name.clone(),
                    ));
                }
                SourcePromotionMemberState::ModifiedMemberResolutionRequired => {
                    if resolution.modified.is_none() || resolution.removed.is_some() {
                        return Err(SourcePromotionDraftError::InvalidResolution(
                            existing.member.directory_name.clone(),
                        ));
                    }
                }
                SourcePromotionMemberState::UpstreamMemberRemoved => {
                    match resolution.removed.as_ref() {
                        Some(UpstreamMemberRemovedResolution::Remove) => {
                            if resolution.modified.is_some() {
                                return Err(SourcePromotionDraftError::InvalidResolution(
                                    existing.member.directory_name.clone(),
                                ));
                            }
                        }
                        Some(UpstreamMemberRemovedResolution::LocalLink { target_directory })
                            if !target_directory.trim().is_empty() =>
                        {
                            if resolution.modified.is_some() {
                                return Err(SourcePromotionDraftError::InvalidResolution(
                                    existing.member.directory_name.clone(),
                                ));
                            }
                        }
                        Some(UpstreamMemberRemovedResolution::LocalLink { .. }) => {
                            return Err(SourcePromotionDraftError::EmptyLocalLinkTarget(
                                existing.member.directory_name.clone(),
                            ));
                        }
                        Some(UpstreamMemberRemovedResolution::ExplicitMemberMapping {
                            target_skill_path,
                        }) => {
                            if !target_paths.contains(target_skill_path.as_str()) {
                                return Err(SourcePromotionDraftError::UnknownMappingTarget(
                                    target_skill_path.clone(),
                                ));
                            }
                            if exact_target_paths.contains_key(target_skill_path.as_str())
                                || !claimed_mapping_paths.insert(target_skill_path.clone())
                            {
                                return Err(SourcePromotionDraftError::OccupiedMappingTarget(
                                    target_skill_path.clone(),
                                ));
                            }
                            let current_is_modified = existing.member.current_tree_hash
                                != existing.member.current_baseline_hash;
                            if current_is_modified != resolution.modified.is_some() {
                                return Err(SourcePromotionDraftError::InvalidResolution(
                                    existing.member.directory_name.clone(),
                                ));
                            }
                        }
                        None => {
                            return Err(SourcePromotionDraftError::InvalidResolution(
                                existing.member.directory_name.clone(),
                            ));
                        }
                    }
                }
            }
        }
        for skill_id in supplied.keys() {
            if !existing_ids.contains(skill_id.as_str()) {
                return Err(SourcePromotionDraftError::MismatchedResolution(
                    skill_id.clone(),
                ));
            }
        }
        Ok(ConfirmedSourcePromotion {
            draft: self,
            resolutions: supplied,
        })
    }
}

fn validate_legacy(legacy: &LegacySourcePromotion) -> Result<(), SourcePromotionDraftError> {
    if legacy.remote_id.is_empty() {
        return Err(SourcePromotionDraftError::MissingRemoteId);
    }
    if legacy.canonical_url.is_empty() {
        return Err(SourcePromotionDraftError::MissingCanonicalUrl);
    }
    if legacy.tracking_ref.is_empty() {
        return Err(SourcePromotionDraftError::MissingTrackingRef);
    }
    if legacy.members.is_empty() {
        return Err(SourcePromotionDraftError::EmptyLegacyMembers);
    }
    let mut paths = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for member in &legacy.members {
        if !paths.insert(member.skill_path.as_str()) {
            return Err(SourcePromotionDraftError::DuplicateLegacyPath(
                member.skill_path.clone(),
            ));
        }
        if !ids.insert(member.skill_id.0.as_str()) {
            return Err(SourcePromotionDraftError::DuplicateLegacySkillId(
                member.skill_id.0.clone(),
            ));
        }
    }
    Ok(())
}

fn target_by_path(
    members: &[SourceGroupMember],
) -> Result<BTreeMap<&str, &SourceGroupMember>, SourcePromotionDraftError> {
    let mut by_path = BTreeMap::new();
    for member in members {
        if by_path.insert(member.skill_path.as_str(), member).is_some() {
            return Err(SourcePromotionDraftError::DuplicateTargetPath(
                member.skill_path.clone(),
            ));
        }
    }
    Ok(by_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_member(path: &str, name: &str) -> SourceGroupMember {
        SourceGroupMember {
            directory_name: name.into(),
            display_name: name.into(),
            description: String::new(),
            skill_path: path.into(),
            tree_summary: format!("target-{name}"),
        }
    }

    fn legacy_member(id: &str, path: &str, current: &str) -> LegacySourcePromotionMember {
        LegacySourcePromotionMember {
            skill_id: SkillId(id.into()),
            directory_name: id.into(),
            skill_path: path.into(),
            current_baseline_hash: "baseline".into(),
            current_tree_hash: current.into(),
        }
    }

    fn legacy() -> LegacySourcePromotion {
        LegacySourcePromotion {
            remote_id: "remote-stable".into(),
            canonical_url: "https://github.com/acme/skills".into(),
            tracking_ref: "main".into(),
            members: vec![
                legacy_member("clean", "skills/clean", "baseline"),
                legacy_member("modified", "skills/modified", "changed"),
                legacy_member("removed", "skills/removed", "baseline"),
            ],
        }
    }

    fn preview() -> SourceGroupPreview {
        SourceGroupPreview {
            provider: "github".into(),
            source_url: "https://github.com/acme/skills".into(),
            tracking_ref: "main".into(),
            resolved_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            members: vec![
                source_member("skills/clean", "clean"),
                source_member("skills/modified", "modified"),
                source_member("skills/new", "new"),
            ],
            external_ownership_claims: Vec::new(),
        }
    }

    #[test]
    fn legacy_promotion_keeps_the_remote_id_and_requires_explicit_conflict_choices() {
        let draft = SourcePromotionDraft::build(legacy(), preview()).expect("draft");

        assert_eq!(draft.remote_id, "remote-stable");
        assert_eq!(draft.target_members.len(), 3);
        assert_eq!(
            draft.existing_members[1].state,
            SourcePromotionMemberState::ModifiedMemberResolutionRequired
        );
        assert_eq!(
            draft.existing_members[2].state,
            SourcePromotionMemberState::UpstreamMemberRemoved
        );
        assert!(matches!(
            draft.clone().confirm(Vec::new()),
            Err(SourcePromotionDraftError::MissingResolution(_))
        ));
    }

    #[test]
    fn removed_member_can_only_map_to_a_known_unoccupied_target_path() {
        let draft = SourcePromotionDraft::build(legacy(), preview()).expect("draft");
        let result = draft.clone().confirm(vec![
            SourcePromotionResolution {
                skill_id: SkillId("modified".into()),
                modified: Some(ModifiedMemberResolution::KeepModified),
                removed: None,
            },
            SourcePromotionResolution {
                skill_id: SkillId("removed".into()),
                modified: None,
                removed: Some(UpstreamMemberRemovedResolution::ExplicitMemberMapping {
                    target_skill_path: "skills/clean".into(),
                }),
            },
        ]);
        assert!(matches!(
            result,
            Err(SourcePromotionDraftError::OccupiedMappingTarget(path)) if path == "skills/clean"
        ));

        draft
            .confirm(vec![
                SourcePromotionResolution {
                    skill_id: SkillId("modified".into()),
                    modified: Some(ModifiedMemberResolution::ReplaceWithTarget),
                    removed: None,
                },
                SourcePromotionResolution {
                    skill_id: SkillId("removed".into()),
                    modified: None,
                    removed: Some(UpstreamMemberRemovedResolution::ExplicitMemberMapping {
                        target_skill_path: "skills/new".into(),
                    }),
                },
            ])
            .expect("explicit mapping to a discovered unoccupied path");
    }
}
