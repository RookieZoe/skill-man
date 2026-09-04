//! Whole Git Repository Source confirmation, recovery and Source Undo
//! (ADR-0018, spec §8.4).
//!
//! The preview is deliberately read-only. This service replays that preview
//! at confirmation, freezes one exact Source Release (policy facts, resolved
//! commit and the complete added/current/removed member manifest) in a
//! durable journal, and only then changes Home, the external installer lock
//! and Catalog. The lock's one full-file CAS is the ownership commit point;
//! no post-CAS path fetches Git. Legacy Source Promotion reuses the same
//! transition machinery with a frozen Legacy audit section (ADR-0014).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::core::domain::{SkillId, parse_skill_metadata, skill_identity_key};
use crate::core::git_source::{git_mirror_path, parse_git_source_input, resolve_git_ref};
use crate::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreview, SourceGroupPreviewError,
    SourceGroupPreviewOutcome, SourceGroupPreviewService, SourceTrackingOverride,
};
use crate::core::write_gate::{HomeWriteContext, ProductWriteGuard, WriteGate, WriteGateError};
use crate::seams::clock::{Clock, iso_timestamp, uuid_v4_shape};
use crate::seams::filesystem::{
    FileSystem, FileSystemError, RemoteParentManifest, SourceTransitionJournal,
    SourceTransitionJournalMember, SourceTransitionPhase, SourceTransitionRemovedMember,
};
use crate::seams::installer_lock_store::{
    InstallerLockError, InstallerLockStore, LockEntry, LockReleaseError,
};
use crate::seams::source::{GitSource, SourceError};
use crate::seams::source_promotion_store::{
    LegacySourcePromotionRecord, SourcePromotionMemberOrigin, SourcePromotionMemberRecord,
    SourcePromotionRecord, SourcePromotionRemovedMemberRecord, SourcePromotionStore,
    SourcePromotionStoreError,
};
use crate::seams::source_transition_store::{
    SourceTransitionMemberRecord, SourceTransitionRecord, SourceTransitionStore,
    SourceTransitionStoreError,
};
use crate::seams::source_update_store::{
    SourceMemberPresence, SourceUpdateCurrentSource, SourceUpdateMemberOrigin,
    SourceUpdateMemberRecord, SourceUpdatePreviousMemberRecord, SourceUpdateRecord,
    SourceUpdateRemovedMemberRecord, SourceUpdateStore, SourceUpdateStoreError,
};

const SOURCE_TRANSITION_JOURNAL_VERSION: u32 = 2;
/// The immutable namespace of every Git Source Member snapshot.
const GIT_SKILLS_NAMESPACE: &str = "skills/git";

struct CleanClaimSet {
    lock_path: PathBuf,
    lock_fingerprint: String,
    lock_entries: Vec<LockEntry>,
    canonical_entities: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmSourceTransitionRequest {
    pub source_type: String,
    pub source_url: String,
    /// The frozen Source Tracking Policy / override selected at preview.
    pub tracking_policy: Option<SourceTrackingOverride>,
    /// The Source Group Draft's selected ref. Confirmation refuses if a
    /// fresh read-only preview no longer resolves the same immutable
    /// release.
    pub expected_selected_ref: String,
    pub expected_resolved_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmSourcePromotionRequest {
    /// The unambiguous Legacy parent id; never client-forged facts.
    pub remote_id: String,
    pub source_type: String,
    pub source_url: String,
    pub tracking_policy: Option<SourceTrackingOverride>,
    pub expected_selected_ref: String,
    pub expected_resolved_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmSourceUpdateRequest {
    /// The durable managed source id; never client-forged facts.
    pub remote_id: String,
    /// The Source Tracking Policy / override selected at preview.
    pub tracking_policy: Option<SourceTrackingOverride>,
    /// The Source Group Draft's selected ref. Confirmation refuses if a
    /// fresh read-only preview no longer resolves the same immutable
    /// release.
    pub expected_selected_ref: String,
    pub expected_resolved_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransitionResult {
    pub operation_id: String,
    pub remote_id: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub member_count: u32,
    pub snapshot_version: u64,
    pub undo_available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUndoResult {
    pub operation_id: String,
    pub member_count: u32,
    pub snapshot_version: u64,
}

#[derive(Debug, Error)]
pub enum SourceTransitionError {
    #[error("{0}")]
    Validation(String),
    #[error("the Source Group Draft is stale; Fetch Latest and Manage again before confirming")]
    PreviewStale,
    #[error("the Source Transition requires recovery: {0}")]
    RecoveryRequired(String),
    #[error(
        "the Git Source Member snapshots do not match the current Source Release; Restore Current Source Release before Update"
    )]
    SourceSnapshotMismatch,
    #[error(
        "the external installer reappeared for this repository; Update is an Ownership Conflict"
    )]
    ExternalOwnershipReappeared,
    #[error(transparent)]
    Preview(#[from] SourceGroupPreviewError),
    #[error(transparent)]
    Lock(#[from] InstallerLockError),
    #[error(transparent)]
    LockRelease(#[from] LockReleaseError),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error(transparent)]
    Store(#[from] SourceTransitionStoreError),
    #[error(transparent)]
    UpdateStore(#[from] SourceUpdateStoreError),
    #[error(transparent)]
    PromotionStore(#[from] SourcePromotionStoreError),
}

/// Core-only orchestration. The client sends only immutable preview facts;
/// all lock entries, filesystem paths and Catalog records are discovered and
/// frozen here instead of being trusted from a DTO.
pub struct SourceTransitionService {
    preview: Arc<SourceGroupPreviewService>,
    git_source: Arc<dyn GitSource>,
    lock_store: Arc<dyn InstallerLockStore>,
    store: Arc<dyn SourceTransitionStore>,
    promotion_store: Arc<dyn SourcePromotionStore>,
    update_store: Arc<dyn SourceUpdateStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    configured_library_root: PathBuf,
    home_directory: PathBuf,
    home_context: Option<Arc<WriteGate>>,
    write_gate: Arc<WriteGate>,
    next_id: AtomicU64,
}

impl SourceTransitionService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        preview: Arc<SourceGroupPreviewService>,
        git_source: Arc<dyn GitSource>,
        lock_store: Arc<dyn InstallerLockStore>,
        store: Arc<dyn SourceTransitionStore>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        library_root: PathBuf,
        home_directory: PathBuf,
        write_gate: Arc<WriteGate>,
    ) -> Self {
        Self {
            preview,
            git_source,
            lock_store,
            store,
            promotion_store: Arc::new(UnavailableSourcePromotionStore),
            update_store: Arc::new(UnavailableSourceUpdateStore),
            filesystem,
            clock,
            configured_library_root: library_root,
            home_directory,
            home_context: None,
            write_gate,
            next_id: AtomicU64::new(1),
        }
    }

    pub(crate) fn write_gate(&self) -> Arc<WriteGate> {
        self.write_gate.clone()
    }

    pub fn with_home_context(mut self, home_context: Arc<WriteGate>) -> Self {
        self.home_context = Some(home_context.clone());
        self.write_gate = home_context;
        self
    }

    pub fn with_promotion_store(mut self, store: Arc<dyn SourcePromotionStore>) -> Self {
        self.promotion_store = store;
        self
    }

    /// Shared handle for the sibling lifecycle service; the same concrete
    /// store backs both seams.
    pub fn confirm(
        &self,
        request: ConfirmSourceTransitionRequest,
    ) -> Result<SourceTransitionResult, SourceTransitionError> {
        let write_context = self.capture_write_context()?;
        let preview = self.refresh_preview(
            &request.source_type,
            &request.source_url,
            &request.tracking_policy,
        )?;
        if preview.policy.selected_ref != request.expected_selected_ref
            || preview.policy.resolved_commit != request.expected_resolved_commit
        {
            return Err(SourceTransitionError::PreviewStale);
        }
        if self
            .store
            .existing_current_members(&preview.source_url)?
            .is_some()
        {
            return Err(SourceTransitionError::Validation(
                "the Git Repository Source is already managed; use Update".into(),
            ));
        }
        let library_root = self.library_root_for_context(&write_context)?;
        let claims = self.clean_claim_set(&preview, true)?;
        let operation_id = self.next_operation_id();
        let remote_id = self.next_uuid();
        let release_id = format!("source-release-{operation_id}");
        let staging_operation_root = library_root.join("staging").join(&operation_id);
        let mut journal = SourceTransitionJournal {
            version: SOURCE_TRANSITION_JOURNAL_VERSION,
            operation_id: operation_id.clone(),
            phase: SourceTransitionPhase::Planned,
            staging_operation_root,
            staging_fingerprint: None,
            remote_id: remote_id.clone(),
            release_id: release_id.clone(),
            provider: preview.provider.clone(),
            canonical_url: preview.source_url.clone(),
            tracking_mode: preview.policy.mode.clone(),
            tracking_value: preview.policy.value.clone(),
            selection_kind: preview.policy.selection_kind.clone(),
            selected_ref: preview.policy.selected_ref.clone(),
            resolved_commit: preview.policy.resolved_commit.clone(),
            target_manifest: Some(RemoteParentManifest {
                schema_version: 1,
                remote_id: remote_id.clone(),
                canonical_url: preview.source_url.clone(),
                provider: Some(preview.provider.clone()),
                tracking_mode: Some(preview.policy.mode.clone()),
                tracking_value: preview.policy.value.clone(),
                current_selected_ref: Some(preview.policy.selected_ref.clone()),
                current_release_id: Some(release_id.clone()),
                aliases: preview.aliases.clone(),
                created_at: iso_timestamp(self.clock.unix_epoch_nanos()),
            }),
            lock_path: claims.lock_path,
            lock_fingerprint: claims.lock_fingerprint,
            lock_entries: claims.lock_entries,
            members: preview
                .members
                .iter()
                .map(|member| {
                    let skill_id = self.next_uuid();
                    SourceTransitionJournalMember {
                        skill_id: skill_id.clone(),
                        directory_name: member.directory_name.clone(),
                        identity_key: member.directory_identity_key.clone(),
                        display_name: member.display_name.clone(),
                        description: member.description.clone(),
                        canonical_entity: claims
                            .canonical_entities
                            .get(&member.directory_name)
                            .cloned(),
                        staged_root: library_root
                            .join("staging")
                            .join(&operation_id)
                            .join(&member.directory_name),
                        isolated_path: None,
                        staged_snapshot: None,
                        namespace_path: git_member_namespace_path(
                            &library_root,
                            &remote_id,
                            &skill_id,
                        ),
                        skill_path: member.skill_path.clone(),
                        tree_hash: String::new(),
                        provider_hash: None,
                        action: crate::seams::filesystem::SourceTransitionMemberAction::Added,
                    }
                })
                .collect(),
            removed_members: Vec::new(),
            promotion_legacy: None,
            update_previous: None,
        };
        let _write_guard = self.acquire_write_guard(&write_context)?;
        self.persist_transition_intent(&library_root, &journal)?;

        let result = self.apply_confirmed_transition(&library_root, &mut journal);
        match result {
            Ok(snapshot_version) => Ok(SourceTransitionResult {
                operation_id,
                remote_id,
                release_id,
                resolved_commit: preview.policy.resolved_commit,
                member_count: u32::try_from(journal.members.len()).map_err(|_| {
                    SourceTransitionError::Validation("too many source members".into())
                })?,
                snapshot_version,
                undo_available: true,
            }),
            Err(error)
                if matches!(
                    journal.phase,
                    SourceTransitionPhase::Planned
                        | SourceTransitionPhase::MembersStaged
                        | SourceTransitionPhase::SourceIsolated
                        | SourceTransitionPhase::DestinationsReserved
                ) =>
            {
                if let Err(compensation) = self.rollback_pre_commit(&library_root, &mut journal) {
                    return Err(
                        self.block_for_recovery("roll back failed Source Transition", compensation)
                    );
                }
                if let Err(archive) = self
                    .filesystem
                    .finish_source_transition_journal(&library_root, &journal.operation_id)
                {
                    return Err(self
                        .block_for_recovery("archive failed Source Transition journal", archive));
                }
                Err(error)
            }
            Err(error) => Err(self.block_for_recovery("continue Source Transition", error)),
        }
    }

    pub fn confirm_promotion(
        &self,
        request: ConfirmSourcePromotionRequest,
    ) -> Result<SourceTransitionResult, SourceTransitionError> {
        let write_context = self.capture_write_context()?;
        let legacy = self
            .promotion_store
            .read_legacy_source_promotion(&request.remote_id)
            .map_err(|error| SourceTransitionError::Validation(error.to_string()))?;
        if legacy.remote_id != request.remote_id || legacy.canonical_url != request.source_url {
            return Err(SourceTransitionError::Validation(
                "the Legacy parent does not match the requested source".into(),
            ));
        }
        if self
            .store
            .existing_current_members(&legacy.canonical_url)?
            .is_some()
        {
            return Err(SourceTransitionError::Validation(
                "the Legacy parent is already a Git Repository Source".into(),
            ));
        }
        let preview = self.refresh_preview(
            &request.source_type,
            &legacy.canonical_url,
            &request.tracking_policy,
        )?;
        if preview.policy.selected_ref != request.expected_selected_ref
            || preview.policy.resolved_commit != request.expected_resolved_commit
        {
            return Err(SourceTransitionError::PreviewStale);
        }
        let library_root = self.library_root_for_context(&write_context)?;
        let claims = self.clean_claim_set(&preview, false)?;
        let operation_id = self.next_operation_id();
        let release_id = format!("source-release-{operation_id}");

        // Match the discovered release against the frozen Legacy members by
        // exact repository-relative `skill_path` (no rewrite/rename guessing).
        let legacy_by_path = legacy
            .members
            .iter()
            .map(|member| (member.skill_path.as_str(), member))
            .collect::<BTreeMap<_, _>>();
        let target_paths = preview
            .members
            .iter()
            .map(|member| member.skill_path.as_str())
            .collect::<BTreeSet<_>>();
        let removed_legacy = legacy
            .members
            .iter()
            .filter(|member| !target_paths.contains(member.skill_path.as_str()))
            .collect::<Vec<_>>();

        let staging_root = library_root.join("staging").join(&operation_id);
        let mut members = Vec::with_capacity(preview.members.len());
        for member in &preview.members {
            let (skill_id, action) = match legacy_by_path.get(member.skill_path.as_str()) {
                Some(legacy_member) => (
                    legacy_member.skill_id.clone(),
                    crate::seams::filesystem::SourceTransitionMemberAction::Current,
                ),
                None => (
                    SkillId(self.next_uuid()),
                    crate::seams::filesystem::SourceTransitionMemberAction::Added,
                ),
            };
            members.push(SourceTransitionJournalMember {
                skill_id: skill_id.0.clone(),
                directory_name: member.directory_name.clone(),
                identity_key: member.directory_identity_key.clone(),
                display_name: member.display_name.clone(),
                description: member.description.clone(),
                canonical_entity: claims
                    .canonical_entities
                    .get(&member.directory_name)
                    .cloned(),
                staged_root: staging_root.join(&member.directory_name),
                isolated_path: None,
                staged_snapshot: None,
                namespace_path: git_member_namespace_path(
                    &library_root,
                    &legacy.remote_id,
                    &skill_id.0,
                ),
                skill_path: member.skill_path.clone(),
                tree_hash: String::new(),
                provider_hash: None,
                action,
            });
        }
        let removed_members = removed_legacy
            .into_iter()
            .map(|member| SourceTransitionRemovedMember {
                skill_id: member.skill_id.0.clone(),
                directory_name: member.directory_name.clone(),
                skill_path: member.skill_path.clone(),
                legacy_path: member.final_entity_path.clone(),
                tree_hash: member.current_baseline_hash.clone(),
                isolated_path: None,
            })
            .collect();
        let mut journal = SourceTransitionJournal {
            version: SOURCE_TRANSITION_JOURNAL_VERSION,
            operation_id: operation_id.clone(),
            phase: SourceTransitionPhase::Planned,
            staging_operation_root: staging_root.clone(),
            staging_fingerprint: None,
            remote_id: legacy.remote_id.clone(),
            release_id: release_id.clone(),
            provider: preview.provider.clone(),
            canonical_url: preview.source_url.clone(),
            tracking_mode: preview.policy.mode.clone(),
            tracking_value: preview.policy.value.clone(),
            selection_kind: preview.policy.selection_kind.clone(),
            selected_ref: preview.policy.selected_ref.clone(),
            resolved_commit: preview.policy.resolved_commit.clone(),
            target_manifest: Some(RemoteParentManifest {
                schema_version: 1,
                remote_id: legacy.remote_id.clone(),
                canonical_url: preview.source_url.clone(),
                provider: Some(preview.provider.clone()),
                tracking_mode: Some(preview.policy.mode.clone()),
                tracking_value: preview.policy.value.clone(),
                current_selected_ref: Some(preview.policy.selected_ref.clone()),
                current_release_id: Some(release_id.clone()),
                aliases: legacy.aliases.clone(),
                created_at: iso_timestamp(self.clock.unix_epoch_nanos()),
            }),
            lock_path: claims.lock_path,
            lock_fingerprint: claims.lock_fingerprint,
            lock_entries: claims.lock_entries,
            members,
            removed_members,
            promotion_legacy: Some(legacy),
            update_previous: None,
        };
        let _write_guard = self.acquire_write_guard(&write_context)?;
        self.persist_transition_intent(&library_root, &journal)?;

        let result = self.apply_confirmed_transition(&library_root, &mut journal);
        match result {
            Ok(snapshot_version) => Ok(SourceTransitionResult {
                operation_id,
                remote_id: journal.remote_id.clone(),
                release_id,
                resolved_commit: preview.policy.resolved_commit,
                member_count: u32::try_from(journal.members.len()).map_err(|_| {
                    SourceTransitionError::Validation("too many source members".into())
                })?,
                snapshot_version,
                undo_available: true,
            }),
            Err(error)
                if matches!(
                    journal.phase,
                    SourceTransitionPhase::Planned
                        | SourceTransitionPhase::MembersStaged
                        | SourceTransitionPhase::SourceIsolated
                        | SourceTransitionPhase::DestinationsReserved
                ) =>
            {
                if let Err(compensation) = self.rollback_pre_commit(&library_root, &mut journal) {
                    return Err(
                        self.block_for_recovery("roll back failed Source Promotion", compensation)
                    );
                }
                if let Err(archive) = self
                    .filesystem
                    .finish_source_transition_journal(&library_root, &journal.operation_id)
                {
                    return Err(
                        self.block_for_recovery("archive failed Source Promotion journal", archive)
                    );
                }
                Err(error)
            }
            Err(error) => Err(self.block_for_recovery("continue Source Promotion", error)),
        }
    }

    /// v9 Source Update (ticket #93): re-evaluates the policy, freezes the
    /// exact previous release/member facts and the complete target release,
    /// verifies the current snapshots and ownership silence, then runs the
    /// same source-level journal/CAS-style transition with no external lock
    /// claims. Members are never updated individually.
    pub fn with_update_store(mut self, store: Arc<dyn SourceUpdateStore>) -> Self {
        self.update_store = store;
        self
    }

    /// Shared handle for sibling lifecycle services; the same concrete
    /// store backs both seams.
    pub fn update_store_handle(&self) -> Arc<dyn SourceUpdateStore> {
        self.update_store.clone()
    }

    pub fn confirm_update(
        &self,
        request: ConfirmSourceUpdateRequest,
    ) -> Result<SourceTransitionResult, SourceTransitionError> {
        let write_context = self.capture_write_context()?;
        let library_root = self.library_root_for_context(&write_context)?;
        let current = self
            .update_store
            .read_current(&request.remote_id)?
            .ok_or_else(|| {
                SourceTransitionError::Validation(
                    "the selected Git Repository Source is no longer complete".into(),
                )
            })?;
        self.ensure_no_external_claims(&current.canonical_url)?;
        self.verify_current_snapshots(&library_root, &current)?;
        let source_type = if current.canonical_url.starts_with("https://github.com/") {
            "github"
        } else if current.canonical_url.starts_with("https://gitlab.com/") {
            "gitlab"
        } else {
            "git"
        };
        // An Update without an explicit override re-evaluates the source's
        // own persisted Source Tracking Policy (spec §8.4).
        let effective_policy = request.tracking_policy.clone().or_else(|| {
            Some(crate::core::source_group_preview::SourceTrackingOverride {
                mode: current.tracking_mode.clone(),
                value: current.tracking_value.clone(),
            })
        });
        let preview =
            self.refresh_preview(source_type, &current.canonical_url, &effective_policy)?;
        if preview.policy.selected_ref != request.expected_selected_ref
            || preview.policy.resolved_commit != request.expected_resolved_commit
        {
            return Err(SourceTransitionError::PreviewStale);
        }
        let operation_id = self.next_operation_id();
        let release_id = format!("source-release-{operation_id}");
        let staging_operation_root = library_root.join("staging").join(&operation_id);
        let previous_manifest = self
            .filesystem
            .read_remote_parent_manifest(&library_root.join("remotes"), &request.remote_id)?;
        let current_by_path = current
            .members
            .iter()
            .map(|member| (member.skill_path.clone(), member))
            .collect::<BTreeMap<_, _>>();
        let target_paths = preview
            .members
            .iter()
            .map(|member| member.skill_path.as_str())
            .collect::<BTreeSet<_>>();
        let removed_current = current
            .members
            .iter()
            .filter(|member| {
                member.presence == SourceMemberPresence::Current
                    && !target_paths.contains(member.skill_path.as_str())
            })
            .collect::<Vec<_>>();
        let mut members = Vec::with_capacity(preview.members.len());
        for member in &preview.members {
            let (skill_id, action) = match current_by_path.get(member.skill_path.as_str()) {
                Some(existing) => (
                    existing.skill_id.clone(),
                    crate::seams::filesystem::SourceTransitionMemberAction::Current,
                ),
                None => (
                    SkillId(self.next_uuid()),
                    crate::seams::filesystem::SourceTransitionMemberAction::Added,
                ),
            };
            members.push(SourceTransitionJournalMember {
                skill_id: skill_id.0.clone(),
                directory_name: member.directory_name.clone(),
                identity_key: member.directory_identity_key.clone(),
                display_name: member.display_name.clone(),
                description: member.description.clone(),
                canonical_entity: None,
                isolated_path: None,
                staged_root: staging_operation_root.join(&member.directory_name),
                staged_snapshot: None,
                namespace_path: git_member_namespace_path(
                    &library_root,
                    &current.remote_id,
                    &skill_id.0,
                ),
                skill_path: member.skill_path.clone(),
                tree_hash: String::new(),
                provider_hash: None,
                action,
            });
        }
        let removed_members = removed_current
            .iter()
            .map(|member| SourceTransitionRemovedMember {
                skill_id: member.skill_id.0.clone(),
                directory_name: member.directory_name.clone(),
                skill_path: member.skill_path.clone(),
                legacy_path: library_root.join(&member.storage_relpath),
                tree_hash: member.tree_hash.clone().unwrap_or_default(),
                isolated_path: None,
            })
            .collect::<Vec<_>>();
        let previous_members = current
            .members
            .iter()
            .map(|member| SourceUpdatePreviousMemberRecord {
                skill_id: member.skill_id.clone(),
                directory_name: member.directory_name.clone(),
                identity_key: member.identity_key.clone(),
                display_name: member.display_name.clone(),
                description: member.description.clone(),
                skill_path: member.skill_path.clone(),
                storage_relpath: member.storage_relpath.clone(),
                presence: member.presence,
                tree_hash: member.tree_hash.clone(),
                health: member.health,
            })
            .collect::<Vec<_>>();
        let update_record = SourceUpdateRecord {
            remote_id: current.remote_id.clone(),
            provider: preview.provider.clone(),
            canonical_url: preview.source_url.clone(),
            tracking_mode: preview.policy.mode.clone(),
            tracking_value: preview.policy.value.clone(),
            selection_kind: preview.policy.selection_kind.clone(),
            selected_ref: preview.policy.selected_ref.clone(),
            release_id: release_id.clone(),
            resolved_commit: preview.policy.resolved_commit.clone(),
            operation_id: operation_id.clone(),
            previous_release_id: current.current_release_id.clone(),
            previous_tracking_mode: current.tracking_mode.clone(),
            previous_tracking_value: current.tracking_value.clone(),
            previous_selected_ref: current.selected_ref.clone(),
            previous_resolved_commit: current.resolved_commit.clone(),
            previous_members,
            members: Vec::new(),
            removed_members: removed_current
                .iter()
                .map(|member| SourceUpdateRemovedMemberRecord {
                    skill_id: member.skill_id.clone(),
                    directory_name: member.directory_name.clone(),
                    skill_path: member.skill_path.clone(),
                    storage_relpath: member.storage_relpath.clone(),
                    previous_tree_hash: member.tree_hash.clone().unwrap_or_default(),
                })
                .collect(),
        };
        let update_previous = crate::seams::filesystem::SourceUpdatePreviousFacts {
            release_id: current.current_release_id.clone(),
            tracking_mode: current.tracking_mode.clone(),
            tracking_value: current.tracking_value.clone(),
            selected_ref: current.selected_ref.clone(),
            resolved_commit: current.resolved_commit.clone(),
            manifest_json: previous_manifest
                .as_ref()
                .and_then(|manifest| serde_json::to_string(manifest).ok()),
            members: current
                .members
                .iter()
                .map(
                    |member| crate::seams::filesystem::SourceUpdatePreviousMemberFacts {
                        skill_id: member.skill_id.0.clone(),
                        directory_name: member.directory_name.clone(),
                        identity_key: member.identity_key.clone(),
                        display_name: member.display_name.clone(),
                        description: member.description.clone(),
                        skill_path: member.skill_path.clone(),
                        storage_relpath: member.storage_relpath.clone(),
                        presence: presence_text(member.presence).into(),
                        tree_hash: member.tree_hash.clone(),
                        health: health_text(member.health).into(),
                    },
                )
                .collect(),
        };
        let mut journal = SourceTransitionJournal {
            version: SOURCE_TRANSITION_JOURNAL_VERSION,
            operation_id: operation_id.clone(),
            phase: SourceTransitionPhase::Planned,
            staging_operation_root,
            staging_fingerprint: None,
            remote_id: current.remote_id.clone(),
            release_id: release_id.clone(),
            provider: preview.provider.clone(),
            canonical_url: preview.source_url.clone(),
            tracking_mode: preview.policy.mode.clone(),
            tracking_value: preview.policy.value.clone(),
            selection_kind: preview.policy.selection_kind.clone(),
            selected_ref: preview.policy.selected_ref.clone(),
            resolved_commit: preview.policy.resolved_commit.clone(),
            target_manifest: Some(RemoteParentManifest {
                schema_version: 1,
                remote_id: current.remote_id.clone(),
                canonical_url: preview.source_url.clone(),
                provider: Some(preview.provider.clone()),
                tracking_mode: Some(preview.policy.mode.clone()),
                tracking_value: preview.policy.value.clone(),
                current_selected_ref: Some(preview.policy.selected_ref.clone()),
                current_release_id: Some(release_id.clone()),
                aliases: current.aliases.clone(),
                created_at: current.created_at.clone(),
            }),
            lock_path: PathBuf::new(),
            lock_fingerprint: String::new(),
            lock_entries: Vec::new(),
            members,
            removed_members,
            promotion_legacy: None,
            update_previous: Some(update_previous.to_json()),
        };
        let _write_guard = self.acquire_write_guard(&write_context)?;
        self.persist_transition_intent(&library_root, &journal)?;
        let mut frozen_record = update_record;

        let result = self.apply_confirmed_update(&library_root, &mut journal, &mut frozen_record);
        match result {
            Ok(snapshot_version) => Ok(SourceTransitionResult {
                operation_id,
                remote_id: journal.remote_id.clone(),
                release_id: journal.release_id.clone(),
                resolved_commit: journal.resolved_commit.clone(),
                member_count: u32::try_from(journal.members.len()).map_err(|_| {
                    SourceTransitionError::Validation("too many source members".into())
                })?,
                snapshot_version,
                undo_available: true,
            }),
            Err(error)
                if matches!(
                    journal.phase,
                    SourceTransitionPhase::Planned
                        | SourceTransitionPhase::MembersStaged
                        | SourceTransitionPhase::SourceIsolated
                        | SourceTransitionPhase::DestinationsReserved
                ) =>
            {
                if let Err(compensation) =
                    self.rollback_pre_commit_update(&library_root, &mut journal, &frozen_record)
                {
                    return Err(
                        self.block_for_recovery("roll back failed Source Update", compensation)
                    );
                }
                if let Err(archive) = self
                    .filesystem
                    .finish_source_transition_journal(&library_root, &journal.operation_id)
                {
                    return Err(
                        self.block_for_recovery("archive failed Source Update journal", archive)
                    );
                }
                Err(error)
            }
            Err(error) => Err(self.block_for_recovery("continue Source Update", error)),
        }
    }

    /// An external installer claim for this repository is an Ownership
    /// Conflict: Update never re-adopts, merges or releases it (spec §8.4).
    fn ensure_no_external_claims(&self, canonical_url: &str) -> Result<(), SourceTransitionError> {
        let reports = self.lock_store.discover()?;
        let reappeared = reports.iter().any(|report| {
            report.entries.iter().any(|entry| {
                parse_git_source_input(&entry.source_url)
                    .map(|spec| spec.url == canonical_url)
                    .unwrap_or(false)
            })
        });
        if reappeared {
            return Err(SourceTransitionError::ExternalOwnershipReappeared);
        }
        Ok(())
    }

    /// Startup and pre-write verification: every current member's read-only
    /// snapshot bytes must equal the immutable current Source Release tree.
    /// Any difference is `SourceSnapshotMismatch`.
    fn verify_current_snapshots(
        &self,
        library_root: &Path,
        current: &SourceUpdateCurrentSource,
    ) -> Result<(), SourceTransitionError> {
        for member in current
            .members
            .iter()
            .filter(|member| member.presence == SourceMemberPresence::Current)
        {
            let namespace = library_root.join(&member.storage_relpath);
            let observed = match self.filesystem.staged_tree_snapshot(&namespace) {
                Ok(snapshot) => Some(snapshot.content_hash),
                Err(FileSystemError::Io { source, .. })
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    None
                }
                Err(error) => return Err(error.into()),
            };
            if observed.as_deref() != member.tree_hash.as_deref() {
                return Err(SourceTransitionError::SourceSnapshotMismatch);
            }
        }
        Ok(())
    }

    /// Startup-only recovery. It reads only the frozen journal: no Fetch,
    /// no remote resolution and no partial source decisions.
    pub fn recover_pending(&self, library_root: &Path) -> Result<(), SourceTransitionError> {
        for mut journal in self
            .filesystem
            .list_source_transition_journals(library_root)?
        {
            self.validate_journal_layout(library_root, &journal)?;
            if journal.update_previous.is_some() {
                match journal.phase {
                    SourceTransitionPhase::Undoing => {
                        self.complete_update_undo(library_root, &mut journal)?;
                    }
                    SourceTransitionPhase::Planned
                    | SourceTransitionPhase::MembersStaged
                    | SourceTransitionPhase::SourceIsolated
                    | SourceTransitionPhase::DestinationsReserved => {
                        let record = self.read_update_record(library_root, &journal)?;
                        if self.update_store.source_update_is_committed(&record)? {
                            self.roll_forward_update(library_root, &mut journal)?;
                        } else {
                            self.rollback_pre_commit_update(library_root, &mut journal, &record)?;
                            self.filesystem.finish_source_transition_journal(
                                library_root,
                                &journal.operation_id,
                            )?;
                        }
                    }
                    SourceTransitionPhase::OwnershipReleased => {
                        return Err(self.block_for_recovery(
                            "decide Source Update recovery",
                            "an update journal must never reach the ownership-release phase",
                        ));
                    }
                    SourceTransitionPhase::ManagedCommitted | SourceTransitionPhase::Finalized => {
                        let record = self.read_update_record(library_root, &journal)?;
                        if !self.update_store.source_update_is_committed(&record)? {
                            return Err(self.block_for_recovery(
                                "decide Source Update recovery",
                                "the update journal is post-commit but the Catalog release is missing",
                            ));
                        }
                        self.roll_forward_update(library_root, &mut journal)?;
                    }
                }
                continue;
            }
            match journal.phase {
                SourceTransitionPhase::Undoing => {
                    self.complete_undo(library_root, &mut journal)?;
                }
                SourceTransitionPhase::Planned
                | SourceTransitionPhase::MembersStaged
                | SourceTransitionPhase::SourceIsolated
                | SourceTransitionPhase::DestinationsReserved => {
                    match self.lock_claim_state(&journal)? {
                        LockClaimState::Present => {
                            self.rollback_pre_commit(library_root, &mut journal)?;
                            self.filesystem.finish_source_transition_journal(
                                library_root,
                                &journal.operation_id,
                            )?;
                        }
                        LockClaimState::Released => {
                            self.roll_forward(library_root, &mut journal)?;
                        }
                    }
                }
                SourceTransitionPhase::OwnershipReleased
                | SourceTransitionPhase::ManagedCommitted
                | SourceTransitionPhase::Finalized => {
                    if self.lock_claim_state(&journal)? != LockClaimState::Released {
                        return Err(self.block_for_recovery(
                            "decide Source Transition recovery",
                            "the journal is post-CAS but the complete claim set is present",
                        ));
                    }
                    self.roll_forward(library_root, &mut journal)?;
                }
            }
        }
        Ok(())
    }

    pub fn undo(&self, operation_id: &str) -> Result<SourceUndoResult, SourceTransitionError> {
        let write_context = self.capture_write_context()?;
        let library_root = self.library_root_for_context(&write_context)?;
        let _write_guard = self.acquire_write_guard(&write_context)?;
        let mut journal = self.journal_for(&library_root, operation_id)?;
        self.validate_journal_layout(&library_root, &journal)?;
        let update_record_present = journal.update_previous.is_some();
        if update_record_present {
            if !matches!(
                journal.phase,
                SourceTransitionPhase::ManagedCommitted | SourceTransitionPhase::Finalized
            ) {
                return Err(SourceTransitionError::Validation(
                    "the Source Update is not in an undoable result window".into(),
                ));
            }
            let record = self.read_update_record(&library_root, &journal)?;
            self.preflight_update_undo(&journal, &record)?;
            journal.phase = SourceTransitionPhase::Undoing;
            self.filesystem
                .write_source_transition_journal(&library_root, &journal)?;
            let snapshot_version = self.complete_update_undo(&library_root, &mut journal)?;
            return Ok(SourceUndoResult {
                operation_id: operation_id.into(),
                member_count: u32::try_from(journal.members.len()).map_err(|_| {
                    SourceTransitionError::Validation("too many source members".into())
                })?,
                snapshot_version,
            });
        }
        if !matches!(
            journal.phase,
            SourceTransitionPhase::ManagedCommitted | SourceTransitionPhase::Finalized
        ) {
            return Err(SourceTransitionError::Validation(
                "the Source Transition is not in an undoable result window".into(),
            ));
        }
        self.preflight_undo(&journal)?;
        journal.phase = SourceTransitionPhase::Undoing;
        self.filesystem
            .write_source_transition_journal(&library_root, &journal)?;
        let snapshot_version = self.complete_undo(&library_root, &mut journal)?;
        Ok(SourceUndoResult {
            operation_id: operation_id.into(),
            member_count: u32::try_from(journal.members.len())
                .map_err(|_| SourceTransitionError::Validation("too many source members".into()))?,
            snapshot_version,
        })
    }

    pub fn finalize(&self, operation_id: &str) -> Result<(), SourceTransitionError> {
        let write_context = self.capture_write_context()?;
        let library_root = self.library_root_for_context(&write_context)?;
        let _write_guard = self.acquire_write_guard(&write_context)?;
        let journal = self.journal_for(&library_root, operation_id)?;
        self.validate_journal_layout(&library_root, &journal)?;
        let update_record_present = journal.update_previous.is_some();
        if update_record_present {
            if !matches!(
                journal.phase,
                SourceTransitionPhase::ManagedCommitted | SourceTransitionPhase::Finalized
            ) {
                return Err(SourceTransitionError::Validation(
                    "the Source Update is not ready to finalize".into(),
                ));
            }
            for member in &journal.members {
                if let Some(isolated) = &member.isolated_path
                    && self.filesystem.path_is_directory(isolated)?
                {
                    self.filesystem.discard_isolated_source(isolated)?;
                }
            }
            for removed in &journal.removed_members {
                if let Some(isolated) = &removed.isolated_path
                    && self.filesystem.path_is_directory(isolated)?
                {
                    self.filesystem.discard_isolated_source(isolated)?;
                }
            }
            self.discard_transition_staging(&library_root, &journal)?;
            self.filesystem
                .finish_source_transition_journal(&library_root, operation_id)?;
            return Ok(());
        }
        if !matches!(
            journal.phase,
            SourceTransitionPhase::ManagedCommitted | SourceTransitionPhase::Finalized
        ) {
            return Err(SourceTransitionError::Validation(
                "the Source Transition is not ready to finalize".into(),
            ));
        }
        for member in &journal.members {
            if let Some(isolated) = &member.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.discard_isolated_source(isolated)?;
            }
        }
        for removed in &journal.removed_members {
            if let Some(isolated) = &removed.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.discard_isolated_source(isolated)?;
            }
        }
        self.discard_transition_staging(&library_root, &journal)?;
        self.filesystem
            .finish_source_transition_journal(&library_root, operation_id)?;
        Ok(())
    }

    fn refresh_preview(
        &self,
        source_type: &str,
        source_url: &str,
        tracking_policy: &Option<SourceTrackingOverride>,
    ) -> Result<SourceGroupPreview, SourceTransitionError> {
        let outcome = self
            .preview
            .fetch_latest_and_manage(FetchLatestAndManageRequest {
                source_type: source_type.into(),
                source_url: source_url.into(),
                tracking_policy: tracking_policy.clone(),
            })?;
        match outcome {
            SourceGroupPreviewOutcome::Preview(preview) => Ok(preview),
            SourceGroupPreviewOutcome::RepositoryRefConflict(_) => Err(
                SourceTransitionError::Validation("the source ref is still conflicted".into()),
            ),
            SourceGroupPreviewOutcome::RepositoryOwnershipSplit(_) => {
                Err(SourceTransitionError::Validation(
                    "the source's external ownership is split across lock files".into(),
                ))
            }
        }
    }

    fn apply_confirmed_transition(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<u64, SourceTransitionError> {
        let operation_fingerprint = self
            .filesystem
            .create_adopt_staging_operation(library_root, &journal.operation_id)?;
        journal.staging_fingerprint = Some(operation_fingerprint);
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;

        // A fresh temporary mirror is safe before CAS. Its only durable
        // output is the staged, validated release recorded in the journal.
        let temporary_mirror_root = tempfile::Builder::new()
            .prefix("skill-man-source-transition-")
            .tempdir()
            .map_err(|source| {
                SourceTransitionError::Source(SourceError::Io {
                    operation: "create temporary Git transition mirror",
                    path: std::env::temp_dir(),
                    source,
                })
            })?;
        let mut spec = parse_git_source_input(&journal.canonical_url)
            .map_err(|error| SourceTransitionError::Validation(error.to_string()))?;
        spec.requested_ref = Some(journal.selected_ref.clone());
        let mirror = git_mirror_path(temporary_mirror_root.path(), &spec.url);
        let report = self.git_source.fetch_mirror(&spec.url, &mirror)?;
        let resolved = resolve_git_ref(self.git_source.as_ref(), &mirror, &spec, &report)
            .map_err(|error| SourceTransitionError::Validation(error.to_string()))?;
        if resolved.commit != journal.resolved_commit {
            return Err(SourceTransitionError::PreviewStale);
        }

        let entry_by_name = journal
            .lock_entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        for index in 0..journal.members.len() {
            {
                let member = &mut journal.members[index];
                self.git_source.stage_skill(
                    &mirror,
                    &journal.resolved_commit,
                    &member.skill_path,
                    &member.staged_root,
                )?;
                let snapshot = self.filesystem.staged_tree_snapshot(&member.staged_root)?;
                crate::core::import::validate_staged_tree(self.filesystem.as_ref(), &snapshot)
                    .map_err(|error| SourceTransitionError::Validation(error.to_string()))?;
                let document = self.filesystem.read_skill_document(&member.staged_root)?;
                let metadata = parse_skill_metadata(&document);
                member.display_name = metadata
                    .name
                    .unwrap_or_else(|| member.directory_name.clone());
                member.description = metadata.description.unwrap_or_default();
                member.tree_hash = snapshot.content_hash.clone();
                member.provider_hash = entry_by_name
                    .get(member.directory_name.as_str())
                    .map(|entry| entry.skill_folder_hash.clone());
                member.staged_snapshot = Some(snapshot);
            }
            self.filesystem
                .write_source_transition_journal(library_root, journal)?;
        }
        // Freeze the removed legacy member tree facts before isolation.
        for index in 0..journal.removed_members.len() {
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&journal.removed_members[index].legacy_path)?;
            journal.removed_members[index].tree_hash = snapshot.content_hash.clone();
        }
        journal.phase = SourceTransitionPhase::MembersStaged;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;

        let record = record_from_journal(journal)?;
        let promotion_record = promotion_record_from_journal(journal)?;
        match &promotion_record {
            None => self.store.validate_new_source_transition(&record)?,
            Some(promotion) => self.promotion_store.validate_source_promotion(promotion)?,
        }
        self.ensure_external_trees_match(journal)?;
        for index in 0..journal.members.len() {
            {
                let member = &mut journal.members[index];
                if let Some(canonical_entity) = &member.canonical_entity {
                    member.isolated_path = Some(
                        self.filesystem
                            .isolate_external_source(canonical_entity, &journal.operation_id)?,
                    );
                }
            }
            self.filesystem
                .write_source_transition_journal(library_root, journal)?;
        }
        for index in 0..journal.removed_members.len() {
            let isolated = self
                .filesystem
                .isolate_external_source(
                    &journal.removed_members[index].legacy_path,
                    &journal.operation_id,
                )
                .map_err(|error| {
                    SourceTransitionError::Validation(format!(
                        "the removed legacy member '{}' cannot be isolated: {error}",
                        journal.removed_members[index].directory_name
                    ))
                })?;
            journal.removed_members[index].isolated_path = Some(isolated);
            self.filesystem
                .write_source_transition_journal(library_root, journal)?;
        }
        self.ensure_isolated_trees_match(journal)?;
        journal.phase = SourceTransitionPhase::SourceIsolated;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.preflight_final_destinations_unoccupied(journal)?;
        // Reserve every destination before the ownership CAS. This makes a
        // new Home occupant a pre-commit conflict instead of discovering it
        // only after external ownership has been released. A pre-CAS crash
        // records DestinationsReserved first, so recovery rolls every
        // reservation back with the external source.
        journal.phase = SourceTransitionPhase::DestinationsReserved;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.reserve_member_destinations(library_root, journal)?;
        self.ensure_reserved_destinations_match(journal)?;

        // The whole claim set is removed in one full-file CAS. From this
        // return onward, failures are recovery-only roll-forward failures.
        self.lock_store.release_entries(
            &journal.lock_path,
            &journal.lock_fingerprint,
            &journal.lock_entries,
        )?;
        journal.phase = SourceTransitionPhase::OwnershipReleased;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;

        self.publish_members(library_root, journal)?;
        self.discard_removed_legacy_entities(journal)?;
        let snapshot_version = match promotion_record {
            None => self.store.commit_source_transition(record)?,
            Some(promotion) => self.promotion_store.commit_source_promotion(promotion)?,
        };
        journal.phase = SourceTransitionPhase::ManagedCommitted;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.write_current_source_manifest(library_root, journal)?;
        self.verify_final_source(journal)?;
        journal.phase = SourceTransitionPhase::Finalized;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        Ok(snapshot_version)
    }

    /// The Source Update apply: stage the frozen release, validate the
    /// complete record, isolate the previous member bytes, reserve the
    /// immutable destinations and commit one source-level Catalog
    /// transaction. No external lock claims exist for an update.
    fn apply_confirmed_update(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
        record: &mut SourceUpdateRecord,
    ) -> Result<u64, SourceTransitionError> {
        let operation_fingerprint = self
            .filesystem
            .create_adopt_staging_operation(library_root, &journal.operation_id)?;
        journal.staging_fingerprint = Some(operation_fingerprint);
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;

        let temporary_mirror_root = tempfile::Builder::new()
            .prefix("skill-man-source-update-")
            .tempdir()
            .map_err(|source| {
                SourceTransitionError::Source(SourceError::Io {
                    operation: "create temporary Git Update mirror",
                    path: std::env::temp_dir(),
                    source,
                })
            })?;
        let mut spec = parse_git_source_input(&journal.canonical_url)
            .map_err(|error| SourceTransitionError::Validation(error.to_string()))?;
        spec.requested_ref = Some(journal.selected_ref.clone());
        let mirror = git_mirror_path(temporary_mirror_root.path(), &spec.url);
        let report = self.git_source.fetch_mirror(&spec.url, &mirror)?;
        let resolved = resolve_git_ref(self.git_source.as_ref(), &mirror, &spec, &report)
            .map_err(|error| SourceTransitionError::Validation(error.to_string()))?;
        if resolved.commit != journal.resolved_commit {
            return Err(SourceTransitionError::PreviewStale);
        }

        for index in 0..journal.members.len() {
            {
                let member = &mut journal.members[index];
                self.git_source.stage_skill(
                    &mirror,
                    &journal.resolved_commit,
                    &member.skill_path,
                    &member.staged_root,
                )?;
                let snapshot = self.filesystem.staged_tree_snapshot(&member.staged_root)?;
                crate::core::import::validate_staged_tree(self.filesystem.as_ref(), &snapshot)
                    .map_err(|error| SourceTransitionError::Validation(error.to_string()))?;
                let document = self.filesystem.read_skill_document(&member.staged_root)?;
                let metadata = parse_skill_metadata(&document);
                member.display_name = metadata
                    .name
                    .unwrap_or_else(|| member.directory_name.clone());
                member.description = metadata.description.unwrap_or_default();
                member.tree_hash = snapshot.content_hash.clone();
                member.staged_snapshot = Some(snapshot);
            }
            self.filesystem
                .write_source_transition_journal(library_root, journal)?;
        }
        journal.phase = SourceTransitionPhase::MembersStaged;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;

        record.members = journal
            .members
            .iter()
            .map(|member| SourceUpdateMemberRecord {
                origin: match member.action {
                    crate::seams::filesystem::SourceTransitionMemberAction::Added => {
                        SourceUpdateMemberOrigin::New
                    }
                    crate::seams::filesystem::SourceTransitionMemberAction::Current => {
                        SourceUpdateMemberOrigin::Existing
                    }
                },
                skill_id: SkillId(member.skill_id.clone()),
                directory_name: member.directory_name.clone(),
                identity_key: member.identity_key.clone(),
                display_name: member.display_name.clone(),
                description: member.description.clone(),
                storage_relpath: format!("skills/git/{}/{}", journal.remote_id, member.skill_id),
                skill_path: member.skill_path.clone(),
                tree_hash: member.tree_hash.clone(),
                provider_hash: member.provider_hash.clone(),
            })
            .collect();
        self.update_store.validate_source_update(record)?;
        self.isolate_update_previous(library_root, journal, record)?;
        journal.phase = SourceTransitionPhase::SourceIsolated;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;

        self.preflight_update_destinations(journal)?;
        journal.phase = SourceTransitionPhase::DestinationsReserved;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.reserve_update_destinations(library_root, journal)?;
        self.ensure_reserved_update_destinations(journal)?;

        // The single transaction is the Update commit point.
        let snapshot_version = self.update_store.commit_source_update(record)?;
        journal.phase = SourceTransitionPhase::ManagedCommitted;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.write_current_source_manifest(library_root, journal)?;
        self.verify_final_update(journal)?;
        journal.phase = SourceTransitionPhase::Finalized;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        Ok(snapshot_version)
    }

    /// Rebuild the frozen complete `SourceUpdateRecord` from the staged
    /// journal facts (freezed tree hashes after staging).
    fn read_update_record(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<SourceUpdateRecord, SourceTransitionError> {
        let previous_value = journal.update_previous.as_ref().ok_or_else(|| {
            SourceTransitionError::RecoveryRequired(
                "the Source Update journal carries no frozen previous facts".into(),
            )
        })?;
        let previous: crate::seams::filesystem::SourceUpdatePreviousFacts =
            crate::seams::filesystem::SourceUpdatePreviousFacts::from_json(previous_value)
                .map_err(SourceTransitionError::RecoveryRequired)?;
        let previous_members = previous
            .members
            .iter()
            .map(|member| SourceUpdatePreviousMemberRecord {
                skill_id: SkillId(member.skill_id.clone()),
                directory_name: member.directory_name.clone(),
                identity_key: member.identity_key.clone(),
                display_name: member.display_name.clone(),
                description: member.description.clone(),
                skill_path: member.skill_path.clone(),
                storage_relpath: member.storage_relpath.clone(),
                presence: parse_presence_inner(&member.presence),
                tree_hash: member.tree_hash.clone(),
                health: parse_health_inner(&member.health),
            })
            .collect();
        let _ = library_root;
        Ok(SourceUpdateRecord {
            remote_id: journal.remote_id.clone(),
            provider: journal.provider.clone(),
            canonical_url: journal.canonical_url.clone(),
            tracking_mode: journal.tracking_mode.clone(),
            tracking_value: journal.tracking_value.clone(),
            selection_kind: journal.selection_kind.clone(),
            selected_ref: journal.selected_ref.clone(),
            release_id: journal.release_id.clone(),
            resolved_commit: journal.resolved_commit.clone(),
            operation_id: journal.operation_id.clone(),
            previous_release_id: previous.release_id.clone(),
            previous_tracking_mode: previous.tracking_mode.clone(),
            previous_tracking_value: previous.tracking_value.clone(),
            previous_selected_ref: previous.selected_ref.clone(),
            previous_resolved_commit: previous.resolved_commit.clone(),
            previous_members,
            members: journal
                .members
                .iter()
                .map(|member| SourceUpdateMemberRecord {
                    skill_id: SkillId(member.skill_id.clone()),
                    directory_name: member.directory_name.clone(),
                    identity_key: member.identity_key.clone(),
                    display_name: member.display_name.clone(),
                    description: member.description.clone(),
                    storage_relpath: format!(
                        "skills/git/{}/{}",
                        journal.remote_id, member.skill_id
                    ),
                    skill_path: member.skill_path.clone(),
                    tree_hash: member.tree_hash.clone(),
                    provider_hash: member.provider_hash.clone(),
                    origin: match member.action {
                        crate::seams::filesystem::SourceTransitionMemberAction::Current => {
                            crate::seams::source_update_store::SourceUpdateMemberOrigin::Existing
                        }
                        crate::seams::filesystem::SourceTransitionMemberAction::Added => {
                            crate::seams::source_update_store::SourceUpdateMemberOrigin::New
                        }
                    },
                })
                .collect(),
            removed_members: journal
                .removed_members
                .iter()
                .map(|removed| SourceUpdateRemovedMemberRecord {
                    skill_id: SkillId(removed.skill_id.clone()),
                    directory_name: removed.directory_name.clone(),
                    skill_path: removed.skill_path.clone(),
                    storage_relpath: removed
                        .legacy_path
                        .strip_prefix(library_root)
                        .map(|path| path.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|_| removed.legacy_path.to_string_lossy().into_owned()),
                    previous_tree_hash: removed.tree_hash.clone(),
                })
                .collect(),
        })
    }

    /// Isolate the previous bytes of changed current members and every
    /// removed member. Unchanged current snapshots and reappearing members
    /// (absent before) have nothing to isolate.
    fn isolate_update_previous(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
        record: &SourceUpdateRecord,
    ) -> Result<(), SourceTransitionError> {
        let previous_by_skill = record
            .previous_members
            .iter()
            .map(|previous| (previous.skill_id.0.as_str(), previous))
            .collect::<BTreeMap<_, _>>();
        for index in 0..journal.members.len() {
            let member = &mut journal.members[index];
            if member.action != crate::seams::filesystem::SourceTransitionMemberAction::Current {
                continue;
            }
            if !self.namespace_occupied(&member.namespace_path)? {
                // Reappearing member: no current bytes to preserve.
                continue;
            }
            let current_bytes = self
                .filesystem
                .staged_tree_snapshot(&member.namespace_path)?;
            if current_bytes.content_hash == member.tree_hash {
                continue;
            }
            let previous_tree = previous_by_skill
                .get(member.skill_id.as_str())
                .and_then(|previous| previous.tree_hash.clone())
                .ok_or_else(|| {
                    SourceTransitionError::RecoveryRequired(
                        "the frozen previous member set has no tree facts".into(),
                    )
                })?;
            if current_bytes.content_hash != previous_tree {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the member '{}' changed after the Update plan was frozen",
                    member.directory_name
                )));
            }
            member.isolated_path = Some(
                self.filesystem
                    .isolate_external_source(&member.namespace_path, &journal.operation_id)?,
            );
            self.filesystem
                .write_source_transition_journal(library_root, journal)?;
        }
        for index in 0..journal.removed_members.len() {
            let removed = &mut journal.removed_members[index];
            if !self.namespace_occupied(&removed.legacy_path)? {
                return Err(SourceTransitionError::PreviewStale);
            }
            let current_bytes = self.filesystem.staged_tree_snapshot(&removed.legacy_path)?;
            if current_bytes.content_hash != removed.tree_hash {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the removed member '{}' changed after the Update plan was frozen",
                    removed.directory_name
                )));
            }
            removed.isolated_path = Some(
                self.filesystem
                    .isolate_external_source(&removed.legacy_path, &journal.operation_id)?,
            );
            self.filesystem
                .write_source_transition_journal(library_root, journal)?;
        }
        Ok(())
    }

    fn preflight_update_destinations(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            if self.namespace_occupied(&member.namespace_path)? {
                match member.action {
                    crate::seams::filesystem::SourceTransitionMemberAction::Current => {}
                    crate::seams::filesystem::SourceTransitionMemberAction::Added => {
                        return Err(SourceTransitionError::PreviewStale);
                    }
                }
            }
        }
        Ok(())
    }

    fn reserve_update_destinations(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            if self.namespace_occupied(&member.namespace_path)? {
                let occupied = self
                    .filesystem
                    .staged_tree_snapshot(&member.namespace_path)?;
                if occupied.content_hash != member.tree_hash {
                    return Err(SourceTransitionError::PreviewStale);
                }
                continue;
            }
            let expected = member.staged_snapshot.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the Source Update journal has no staged snapshot for '{}'",
                    member.directory_name
                ))
            })?;
            let staged_snapshot = self.filesystem.staged_tree_snapshot(&member.staged_root)?;
            if staged_snapshot.content_hash != expected.content_hash {
                return Err(SourceTransitionError::PreviewStale);
            }
            self.install_git_snapshot(
                &member.staged_root,
                &member.namespace_path,
                library_root,
                &journal.operation_id,
                expected,
            )?;
        }
        Ok(())
    }

    fn ensure_reserved_update_destinations(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            if !self.filesystem.path_is_directory(&member.namespace_path)? {
                return Err(SourceTransitionError::PreviewStale);
            }
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.namespace_path)?;
            if snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::PreviewStale);
            }
        }
        Ok(())
    }

    fn verify_final_update(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        let expected_manifest = journal.target_manifest.as_ref().ok_or_else(|| {
            SourceTransitionError::RecoveryRequired(
                "the Source Update journal has no frozen source manifest".into(),
            )
        })?;
        if self.filesystem.read_remote_parent_manifest(
            &self.active_library_root()?.join("remotes"),
            &journal.remote_id,
        )? != Some(expected_manifest.clone())
        {
            return Err(SourceTransitionError::RecoveryRequired(
                "the Source Update manifest does not match the fixed Source Release".into(),
            ));
        }
        for member in &journal.members {
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.namespace_path)?;
            if snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "Managed Skill '{}' does not match the fixed Source Release",
                    member.directory_name
                )));
            }
        }
        for removed in &journal.removed_members {
            if self.filesystem.path_is_occupied(&removed.legacy_path)? {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "removed member '{}' reappeared after the commit point",
                    removed.directory_name
                )));
            }
        }
        Ok(())
    }

    fn rollback_pre_commit_update(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
        record: &SourceUpdateRecord,
    ) -> Result<(), SourceTransitionError> {
        let previous_by_skill = record
            .previous_members
            .iter()
            .map(|previous| (previous.skill_id.0.as_str(), previous))
            .collect::<BTreeMap<_, _>>();
        if journal.phase == SourceTransitionPhase::SourceIsolated
            || journal.phase == SourceTransitionPhase::DestinationsReserved
        {
            for member in journal.members.iter_mut().rev() {
                if member.isolated_path.is_some() {
                    if self.namespace_occupied(&member.namespace_path)? {
                        if !self.filesystem.path_is_directory(&member.namespace_path)? {
                            return Err(self.block_for_recovery(
                                "roll back Source Update",
                                format!(
                                    "the reserved Home destination '{}' is no longer a directory",
                                    member.directory_name
                                ),
                            ));
                        }
                        let snapshot = self
                            .filesystem
                            .staged_tree_snapshot(&member.namespace_path)?;
                        if snapshot.content_hash != member.tree_hash {
                            return Err(self.block_for_recovery(
                                "roll back Source Update",
                                format!(
                                    "the reserved Home destination '{}' changed",
                                    member.directory_name
                                ),
                            ));
                        }
                        self.filesystem
                            .remove_directory_verified(&member.namespace_path)?;
                    }
                    let isolated = member.isolated_path.clone().ok_or_else(|| {
                        SourceTransitionError::RecoveryRequired(
                            "the Source Update isolation copy is missing".into(),
                        )
                    })?;
                    let previous_tree = previous_by_skill
                        .get(member.skill_id.as_str())
                        .and_then(|previous| previous.tree_hash.clone())
                        .ok_or_else(|| {
                            SourceTransitionError::RecoveryRequired(
                                "the frozen previous member set has no tree facts".into(),
                            )
                        })?;
                    self.filesystem.restore_isolated_source(
                        &isolated,
                        &member.namespace_path,
                        &previous_tree,
                    )?;
                    member.isolated_path = None;
                } else if member.action
                    == crate::seams::filesystem::SourceTransitionMemberAction::Added
                    && self.namespace_occupied(&member.namespace_path)?
                {
                    let snapshot = self
                        .filesystem
                        .staged_tree_snapshot(&member.namespace_path)?;
                    if snapshot.content_hash != member.tree_hash {
                        return Err(self.block_for_recovery(
                            "roll back Source Update",
                            format!(
                                "the added member '{}' changed during rollback",
                                member.directory_name
                            ),
                        ));
                    }
                    self.filesystem
                        .remove_directory_verified(&member.namespace_path)?;
                }
            }
            for removed in journal.removed_members.iter_mut().rev() {
                if let Some(isolated) = &removed.isolated_path {
                    self.filesystem.restore_isolated_source(
                        isolated,
                        &removed.legacy_path,
                        &removed.tree_hash,
                    )?;
                    removed.isolated_path = None;
                }
            }
        }
        journal.phase = SourceTransitionPhase::Planned;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.discard_transition_staging(library_root, journal)
    }

    /// Post-CAS recovery convergence: install missing destinations and the
    /// Catalog state (idempotently), then close the operation. It never
    /// fetches Git again.
    fn roll_forward_update(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        let record = self.read_update_record(library_root, journal)?;
        if !self.update_store.source_update_is_committed(&record)? {
            self.update_store.commit_source_update(&record)?;
        }
        for member in &journal.members {
            if self.namespace_occupied(&member.namespace_path)? {
                let occupied = self
                    .filesystem
                    .staged_tree_snapshot(&member.namespace_path)?;
                if occupied.content_hash != member.tree_hash {
                    return Err(SourceTransitionError::RecoveryRequired(format!(
                        "the member '{}' changed during recovery",
                        member.directory_name
                    )));
                }
                continue;
            }
            if !self.filesystem.path_is_directory(&member.staged_root)? {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the staged member '{}' is missing during recovery",
                    member.directory_name
                )));
            }
            let staged_snapshot = self.filesystem.staged_tree_snapshot(&member.staged_root)?;
            if staged_snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the staged member '{}' no longer matches the journal",
                    member.directory_name
                )));
            }
            self.install_git_snapshot(
                &member.staged_root,
                &member.namespace_path,
                library_root,
                &journal.operation_id,
                member.staged_snapshot.as_ref().ok_or_else(|| {
                    SourceTransitionError::RecoveryRequired(
                        "the Source Update journal has no staged snapshot".into(),
                    )
                })?,
            )?;
        }
        for removed in &journal.removed_members {
            if self.filesystem.path_is_occupied(&removed.legacy_path)? {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the removed member '{}' reappeared during recovery",
                    removed.directory_name
                )));
            }
        }
        self.freeze_target_manifest_for_recovery(library_root, journal)?;
        self.write_current_source_manifest(library_root, journal)?;
        journal.phase = SourceTransitionPhase::Finalized;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.verify_final_update(journal)?;
        for member in &journal.members {
            if let Some(isolated) = &member.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.discard_isolated_source(isolated)?;
            }
        }
        for removed in &journal.removed_members {
            if let Some(isolated) = &removed.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.discard_isolated_source(isolated)?;
            }
        }
        self.discard_transition_staging(library_root, journal)?;
        self.filesystem
            .finish_source_transition_journal(library_root, &journal.operation_id)?;
        Ok(())
    }

    fn complete_update_undo(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<u64, SourceTransitionError> {
        let record = self.read_update_record(library_root, journal)?;
        if !self.update_store.source_update_is_committed(&record)? {
            return Err(SourceTransitionError::Validation(
                "the Source Update no longer matches the result window".into(),
            ));
        }
        self.preflight_update_undo(journal, &record)?;
        let snapshot_version = self.update_store.undo_source_update(&record)?;
        let previous_by_skill = record
            .previous_members
            .iter()
            .map(|previous| (previous.skill_id.0.as_str(), previous))
            .collect::<BTreeMap<_, _>>();
        for member in &journal.members {
            if let Some(isolated) = &member.isolated_path {
                if self.namespace_occupied(&member.namespace_path)? {
                    let snapshot = self
                        .filesystem
                        .staged_tree_snapshot(&member.namespace_path)?;
                    if snapshot.content_hash != member.tree_hash {
                        return Err(self.block_for_recovery(
                            "complete Source Update Undo",
                            format!("Home member '{}' changed", member.directory_name),
                        ));
                    }
                    self.filesystem
                        .remove_directory_verified(&member.namespace_path)?;
                }
                let previous_tree = previous_by_skill
                    .get(member.skill_id.as_str())
                    .and_then(|previous| previous.tree_hash.clone())
                    .ok_or_else(|| {
                        SourceTransitionError::RecoveryRequired(
                            "the frozen previous member set has no tree facts".into(),
                        )
                    })?;
                self.filesystem.restore_isolated_source(
                    isolated,
                    &member.namespace_path,
                    &previous_tree,
                )?;
            } else if member.action == crate::seams::filesystem::SourceTransitionMemberAction::Added
                && self.namespace_occupied(&member.namespace_path)?
            {
                let snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&member.namespace_path)?;
                if snapshot.content_hash != member.tree_hash {
                    return Err(self.block_for_recovery(
                        "complete Source Update Undo",
                        format!("Home member '{}' changed", member.directory_name),
                    ));
                }
                self.filesystem
                    .remove_directory_verified(&member.namespace_path)?;
            }
        }
        for removed in &journal.removed_members {
            let isolated = removed.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the Source Update Undo journal has no preservation copy for removed member '{}'",
                    removed.directory_name
                ))
            })?;
            if self.filesystem.path_is_occupied(&removed.legacy_path)? {
                return Err(self.block_for_recovery(
                    "complete Source Update Undo",
                    format!(
                        "the removed member '{}' is occupied",
                        removed.directory_name
                    ),
                ));
            }
            self.filesystem.restore_isolated_source(
                isolated,
                &removed.legacy_path,
                &removed.tree_hash,
            )?;
        }
        self.discard_transition_staging(library_root, journal)?;
        self.filesystem
            .finish_source_transition_journal(library_root, &journal.operation_id)?;
        Ok(snapshot_version)
    }

    fn preflight_update_undo(
        &self,
        journal: &SourceTransitionJournal,
        record: &SourceUpdateRecord,
    ) -> Result<(), SourceTransitionError> {
        let previous_by_skill = record
            .previous_members
            .iter()
            .map(|previous| (previous.skill_id.0.as_str(), previous))
            .collect::<BTreeMap<_, _>>();
        for member in &journal.members {
            if self.namespace_occupied(&member.namespace_path)? {
                if !self.filesystem.path_is_directory(&member.namespace_path)? {
                    return Err(SourceTransitionError::Validation(format!(
                        "the Managed Skill '{}' is no longer a directory",
                        member.directory_name
                    )));
                }
                let final_snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&member.namespace_path)?;
                if final_snapshot.content_hash != member.tree_hash {
                    return Err(SourceTransitionError::Validation(format!(
                        "the Managed Skill '{}' changed after the Update",
                        member.directory_name
                    )));
                }
            }
            if let Some(isolated) = &member.isolated_path {
                let snapshot = self.filesystem.staged_tree_snapshot(isolated)?;
                let previous_tree = previous_by_skill
                    .get(member.skill_id.as_str())
                    .and_then(|previous| previous.tree_hash.clone())
                    .ok_or_else(|| {
                        SourceTransitionError::Validation(
                            "the frozen previous member set has no tree facts".into(),
                        )
                    })?;
                if snapshot.content_hash != previous_tree {
                    return Err(SourceTransitionError::Validation(format!(
                        "the preservation copy for '{}' changed",
                        member.directory_name
                    )));
                }
            }
        }
        for removed in &journal.removed_members {
            if self.filesystem.path_is_occupied(&removed.legacy_path)? {
                return Err(SourceTransitionError::Validation(format!(
                    "the removed member '{}' reappeared",
                    removed.directory_name
                )));
            }
            let isolated = removed.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::Validation(format!(
                    "the preservation copy for removed member '{}' is unavailable",
                    removed.directory_name
                ))
            })?;
            let snapshot = self.filesystem.staged_tree_snapshot(isolated)?;
            if snapshot.content_hash != removed.tree_hash {
                return Err(SourceTransitionError::Validation(format!(
                    "the preservation copy for removed member '{}' changed",
                    removed.directory_name
                )));
            }
        }
        Ok(())
    }

    fn roll_forward(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        self.publish_members(library_root, journal)?;
        self.discard_removed_legacy_entities(journal)?;
        let record = record_from_journal(journal)?;
        let promotion_record = promotion_record_from_journal(journal)?;
        let _ = match promotion_record {
            None => self.store.commit_source_transition(record)?,
            Some(promotion) => self.promotion_store.commit_source_promotion(promotion)?,
        };
        self.freeze_target_manifest_for_recovery(library_root, journal)?;
        self.write_current_source_manifest(library_root, journal)?;
        journal.phase = SourceTransitionPhase::Finalized;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.verify_final_source(journal)?;
        for member in &journal.members {
            if let Some(isolated) = &member.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.discard_isolated_source(isolated)?;
            }
        }
        for removed in &journal.removed_members {
            if let Some(isolated) = &removed.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.discard_isolated_source(isolated)?;
            }
        }
        self.discard_transition_staging(library_root, journal)?;
        self.filesystem
            .finish_source_transition_journal(library_root, &journal.operation_id)?;
        Ok(())
    }

    fn publish_members(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            let expected = member.staged_snapshot.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal has no staged snapshot for '{}'",
                    member.directory_name
                ))
            })?;
            if self.filesystem.path_is_directory(&member.namespace_path)? {
                let final_snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&member.namespace_path)?;
                if final_snapshot.content_hash != member.tree_hash {
                    return Err(SourceTransitionError::RecoveryRequired(format!(
                        "the Home member '{}' does not match the frozen Source Release",
                        member.directory_name
                    )));
                }
            } else {
                let staged_snapshot = self.filesystem.staged_tree_snapshot(&member.staged_root)?;
                if staged_snapshot.content_hash != expected.content_hash {
                    return Err(SourceTransitionError::RecoveryRequired(format!(
                        "the staged member '{}' no longer matches the journal",
                        member.directory_name
                    )));
                }
                self.install_git_snapshot(
                    &member.staged_root,
                    &member.namespace_path,
                    library_root,
                    &journal.operation_id,
                    expected,
                )?;
            }
        }
        Ok(())
    }

    /// The removed legacy entities are gone from Home after the commit
    /// point; their isolated copies stay until the Undo window closes.
    fn discard_removed_legacy_entities(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for removed in &journal.removed_members {
            let isolated = removed.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the removed legacy member '{}' has no isolated copy",
                    removed.directory_name
                ))
            })?;
            if self.filesystem.path_is_directory(isolated)? {
                continue;
            }
            if self.filesystem.path_is_occupied(&removed.legacy_path)? {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the removed legacy member '{}' is still present",
                    removed.directory_name
                )));
            }
        }
        Ok(())
    }

    fn rollback_pre_commit(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        if journal.phase == SourceTransitionPhase::DestinationsReserved
            || journal.phase == SourceTransitionPhase::SourceIsolated
        {
            for member in journal.members.iter().rev() {
                if !self.namespace_occupied(&member.namespace_path)? {
                    continue;
                }
                if !self.filesystem.path_is_directory(&member.namespace_path)? {
                    return Err(self.block_for_recovery(
                        "roll back Source Transition",
                        format!(
                            "the reserved Home destination '{}' is no longer a directory",
                            member.directory_name
                        ),
                    ));
                }
                let snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&member.namespace_path)?;
                if snapshot.content_hash != member.tree_hash {
                    return Err(self.block_for_recovery(
                        "roll back Source Transition",
                        format!(
                            "the reserved Home destination '{}' changed",
                            member.directory_name
                        ),
                    ));
                }
                self.filesystem
                    .remove_directory_verified(&member.namespace_path)?;
            }
        }
        for member in journal.members.iter_mut().rev() {
            if let Some(isolated) = &member.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                match &member.canonical_entity {
                    Some(canonical_entity) => {
                        self.filesystem.restore_isolated_source(
                            isolated,
                            canonical_entity,
                            &member.tree_hash,
                        )?;
                    }
                    None => self.filesystem.discard_isolated_source(isolated)?,
                }
            }
            member.isolated_path = None;
        }
        for removed in journal.removed_members.iter_mut().rev() {
            if let Some(isolated) = &removed.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.restore_isolated_source(
                    isolated,
                    &removed.legacy_path,
                    &removed.tree_hash,
                )?;
            }
            removed.isolated_path = None;
        }
        journal.phase = SourceTransitionPhase::Planned;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;
        self.discard_transition_staging(library_root, journal)
    }

    fn complete_undo(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<u64, SourceTransitionError> {
        self.validate_journal_layout(library_root, journal)?;
        let record = record_from_journal(journal)?;
        let promotion_record = promotion_record_from_journal(journal)?;
        let catalog_is_committed = match &promotion_record {
            None => self.store.source_transition_is_committed(&record)?,
            Some(promotion) => self
                .promotion_store
                .source_promotion_is_committed(promotion)?,
        };
        // Revalidate the entire conditional Undo after the durable Undoing
        // cursor exists and immediately before Catalog mutation. A changed
        // Home path, external path, lock or preservation copy is never
        // silently converted into a partial Undo.
        if catalog_is_committed {
            self.preflight_undo(journal)?;
        }
        let snapshot_version = if catalog_is_committed {
            match &promotion_record {
                None => self.store.undo_source_transition(&record)?,
                Some(promotion) => self
                    .promotion_store
                    .undo_source_promotion(promotion, &promotion.legacy)?,
            }
        } else {
            0
        };
        self.filesystem
            .remove_remote_parent_manifest(&library_root.join("remotes"), &journal.remote_id)?;
        for member in &journal.members {
            if self.namespace_occupied(&member.namespace_path)? {
                if !self.filesystem.path_is_directory(&member.namespace_path)? {
                    return Err(self.block_for_recovery(
                        "complete Source Undo",
                        format!(
                            "Home member '{}' is no longer a directory",
                            member.directory_name
                        ),
                    ));
                }
                let final_snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&member.namespace_path)?;
                if final_snapshot.content_hash != member.tree_hash {
                    return Err(self.block_for_recovery(
                        "complete Source Undo",
                        format!("Home member '{}' changed", member.directory_name),
                    ));
                }
                self.filesystem
                    .remove_directory_verified(&member.namespace_path)?;
            }
            // The empty remote namespace directory is part of the source's
            // owned state; a clean Undo removes it with the members.
            if let Some(first) = journal.members.first() {
                if let Some(remote_directory) = first.namespace_path.parent() {
                    if self.filesystem.list_directory(remote_directory)?.is_empty() {
                        self.filesystem
                            .remove_directory_verified(remote_directory)?;
                    }
                }
            }
            match &member.canonical_entity {
                Some(canonical_entity) => {
                    let isolated = member.isolated_path.as_ref().ok_or_else(|| {
                        SourceTransitionError::RecoveryRequired(format!(
                            "the Source Undo journal has no external copy for '{}'",
                            member.directory_name
                        ))
                    })?;
                    if self.filesystem.path_is_occupied(canonical_entity)? {
                        // A previous recovery may already have restored this
                        // member. Only accept that idempotent state when the
                        // preserved copy has gone and the canonical tree is
                        // exactly frozen; an occupied path while its
                        // preservation copy remains is a concurrent external
                        // owner and must remain locked.
                        if self.filesystem.path_is_directory(isolated)?
                            || !self.filesystem.path_is_directory(canonical_entity)?
                        {
                            return Err(self.block_for_recovery(
                                "complete Source Undo",
                                format!(
                                    "the external source location for '{}' is occupied",
                                    member.directory_name
                                ),
                            ));
                        }
                        let canonical_snapshot =
                            self.filesystem.staged_tree_snapshot(canonical_entity)?;
                        if canonical_snapshot.content_hash != member.tree_hash {
                            return Err(self.block_for_recovery(
                                "complete Source Undo",
                                format!(
                                    "the external source location for '{}' changed",
                                    member.directory_name
                                ),
                            ));
                        }
                    } else {
                        if !self.filesystem.path_is_directory(isolated)? {
                            return Err(self.block_for_recovery(
                                "complete Source Undo",
                                format!(
                                    "the external preservation copy for '{}' is missing",
                                    member.directory_name
                                ),
                            ));
                        }
                        self.filesystem.restore_isolated_source(
                            isolated,
                            canonical_entity,
                            &member.tree_hash,
                        )?;
                    }
                }
                None => {
                    // A freshly added source member has no external entity;
                    // its isolated copy (if any) is discarded with the undo.
                    if let Some(isolated) = &member.isolated_path
                        && self.filesystem.path_is_directory(isolated)?
                    {
                        self.filesystem.discard_isolated_source(isolated)?;
                    }
                }
            }
        }
        for removed in &journal.removed_members {
            let isolated = removed.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the Source Undo journal has no preservation copy for removed member '{}'",
                    removed.directory_name
                ))
            })?;
            if self.filesystem.path_is_occupied(&removed.legacy_path)? {
                if self.filesystem.path_is_directory(isolated)?
                    || !self.filesystem.path_is_directory(&removed.legacy_path)?
                {
                    return Err(self.block_for_recovery(
                        "complete Source Undo",
                        format!(
                            "the removed legacy member '{}' is occupied",
                            removed.directory_name
                        ),
                    ));
                }
                let snapshot = self.filesystem.staged_tree_snapshot(&removed.legacy_path)?;
                if snapshot.content_hash != removed.tree_hash {
                    return Err(self.block_for_recovery(
                        "complete Source Undo",
                        format!(
                            "the removed legacy member '{}' changed",
                            removed.directory_name
                        ),
                    ));
                }
            } else {
                if !self.filesystem.path_is_directory(isolated)? {
                    return Err(self.block_for_recovery(
                        "complete Source Undo",
                        format!(
                            "the preservation copy for removed member '{}' is missing",
                            removed.directory_name
                        ),
                    ));
                }
                self.filesystem.restore_isolated_source(
                    isolated,
                    &removed.legacy_path,
                    &removed.tree_hash,
                )?;
            }
        }
        // A crash may happen after the atomic restore but before this journal
        // can be archived. The live complete claim set proves that exact
        // sub-step already happened, so recovery skips a second restore;
        // absent claims are restored once, and a partial set stays closed.
        if self.undo_lock_claim_state(journal)? == LockClaimState::Released {
            self.lock_store
                .restore_entries(&journal.lock_path, &journal.lock_entries)?;
        }
        self.discard_transition_staging(library_root, journal)?;
        self.filesystem
            .finish_source_transition_journal(library_root, &journal.operation_id)?;
        Ok(snapshot_version)
    }

    fn preflight_undo(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        let record = record_from_journal(journal)?;
        let promotion_record = promotion_record_from_journal(journal)?;
        let committed = match &promotion_record {
            None => self.store.source_transition_is_committed(&record)?,
            Some(promotion) => self
                .promotion_store
                .source_promotion_is_committed(promotion)?,
        };
        if !committed {
            return Err(SourceTransitionError::Validation(
                "the Source Release no longer matches the result window".into(),
            ));
        }
        if self.lock_claim_state(journal)? != LockClaimState::Released {
            return Err(SourceTransitionError::Validation(
                "the installer lock claims changed after the Source Transition".into(),
            ));
        }
        for member in &journal.members {
            if let Some(canonical_entity) = &member.canonical_entity
                && self.filesystem.path_is_occupied(canonical_entity)?
            {
                return Err(SourceTransitionError::Validation(format!(
                    "the external source location for '{}' is occupied",
                    member.directory_name
                )));
            }
            if !self.filesystem.path_is_directory(&member.namespace_path)? {
                return Err(SourceTransitionError::Validation(format!(
                    "the Managed Skill '{}' is no longer a directory",
                    member.directory_name
                )));
            }
            let final_snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.namespace_path)?;
            if final_snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::Validation(format!(
                    "the Managed Skill '{}' changed after confirmation",
                    member.directory_name
                )));
            }
            if let Some(isolated) = &member.isolated_path {
                let isolated_snapshot = self.filesystem.staged_tree_snapshot(isolated)?;
                if isolated_snapshot.content_hash != member.tree_hash {
                    return Err(SourceTransitionError::Validation(format!(
                        "the external preservation copy for '{}' changed",
                        member.directory_name
                    )));
                }
            }
        }
        for removed in &journal.removed_members {
            if self.filesystem.path_is_occupied(&removed.legacy_path)? {
                return Err(SourceTransitionError::Validation(format!(
                    "the removed legacy member '{}' reappeared",
                    removed.directory_name
                )));
            }
            let isolated = removed.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::Validation(format!(
                    "the preservation copy for removed member '{}' is unavailable",
                    removed.directory_name
                ))
            })?;
            let snapshot = self.filesystem.staged_tree_snapshot(isolated)?;
            if snapshot.content_hash != removed.tree_hash {
                return Err(SourceTransitionError::Validation(format!(
                    "the preservation copy for removed member '{}' changed",
                    removed.directory_name
                )));
            }
        }
        Ok(())
    }

    /// Occupancy probe for the immutable namespace that treats a namespace
    /// whose hierarchy is not a directory (`ENOTDIR`/`ENOENT` during a
    /// workspace-level collision) as unoccupied; the later install/snapshot
    /// checks still fail closed on the real state.
    fn namespace_occupied(&self, path: &Path) -> Result<bool, SourceTransitionError> {
        match self.filesystem.path_is_occupied(path) {
            Ok(occupied) => Ok(occupied),
            Err(FileSystemError::Io { source, .. })
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn reserve_member_destinations(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            if self.namespace_occupied(&member.namespace_path)? {
                return Err(SourceTransitionError::PreviewStale);
            }
            let expected = member.staged_snapshot.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal has no staged snapshot for '{}'",
                    member.directory_name
                ))
            })?;
            let staged_snapshot = self.filesystem.staged_tree_snapshot(&member.staged_root)?;
            if staged_snapshot.content_hash != expected.content_hash {
                return Err(SourceTransitionError::PreviewStale);
            }
            self.install_git_snapshot(
                &member.staged_root,
                &member.namespace_path,
                library_root,
                &journal.operation_id,
                expected,
            )?;
        }
        Ok(())
    }

    fn preflight_final_destinations_unoccupied(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            if self.namespace_occupied(&member.namespace_path)? {
                return Err(SourceTransitionError::PreviewStale);
            }
        }
        Ok(())
    }

    fn ensure_reserved_destinations_match(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            if !self.filesystem.path_is_directory(&member.namespace_path)? {
                return Err(SourceTransitionError::PreviewStale);
            }
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.namespace_path)?;
            if snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::PreviewStale);
            }
        }
        Ok(())
    }

    fn ensure_external_trees_match(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            let Some(canonical_entity) = &member.canonical_entity else {
                continue;
            };
            let snapshot = self.filesystem.staged_tree_snapshot(canonical_entity)?;
            if snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::Validation(format!(
                    "the external member '{}' does not match the frozen Source Release",
                    member.directory_name
                )));
            }
        }
        Ok(())
    }

    fn ensure_isolated_trees_match(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            if member.canonical_entity.is_none() {
                // A member without an external declaration is never isolated.
                continue;
            }
            let isolated = member.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the isolated external member '{}' is missing from the journal",
                    member.directory_name
                ))
            })?;
            let snapshot = self.filesystem.staged_tree_snapshot(isolated)?;
            if snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::PreviewStale);
            }
        }
        for removed in &journal.removed_members {
            let isolated = removed.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the isolated removed member '{}' is missing from the journal",
                    removed.directory_name
                ))
            })?;
            let snapshot = self.filesystem.staged_tree_snapshot(isolated)?;
            if snapshot.content_hash != removed.tree_hash {
                return Err(SourceTransitionError::PreviewStale);
            }
        }
        Ok(())
    }

    fn verify_final_source(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        let expected_manifest = journal.target_manifest.as_ref().ok_or_else(|| {
            SourceTransitionError::RecoveryRequired(
                "the Source Transition journal has no frozen source manifest".into(),
            )
        })?;
        if self.filesystem.read_remote_parent_manifest(
            &self.active_library_root()?.join("remotes"),
            &journal.remote_id,
        )? != Some(expected_manifest.clone())
        {
            return Err(SourceTransitionError::RecoveryRequired(
                "the Source Transition manifest does not match the fixed Source Release".into(),
            ));
        }
        for member in &journal.members {
            if let Some(canonical_entity) = &member.canonical_entity
                && self.filesystem.path_is_directory(canonical_entity)?
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "external source '{}' reappeared after the ownership commit point",
                    member.directory_name
                )));
            }
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.namespace_path)?;
            if snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "Managed Skill '{}' does not match the fixed Source Release",
                    member.directory_name
                )));
            }
        }
        for removed in &journal.removed_members {
            if self.filesystem.path_is_occupied(&removed.legacy_path)? {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "removed legacy member '{}' reappeared after the commit point",
                    removed.directory_name
                )));
            }
        }
        Ok(())
    }

    fn write_current_source_manifest(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        let manifest = journal.target_manifest.as_ref().ok_or_else(|| {
            SourceTransitionError::RecoveryRequired(
                "the Source Transition journal has no frozen source manifest".into(),
            )
        })?;
        self.filesystem
            .write_remote_parent_manifest(&library_root.join("remotes"), manifest)?;
        Ok(())
    }

    /// Compatibility for an interrupted journal written before the manifest
    /// became a frozen fact. Persist its one generated manifest before
    /// publishing it, so a second restart checks the same bytes.
    fn freeze_target_manifest_for_recovery(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        if journal.target_manifest.is_none() {
            journal.target_manifest = Some(RemoteParentManifest {
                schema_version: 1,
                remote_id: journal.remote_id.clone(),
                canonical_url: journal.canonical_url.clone(),
                provider: Some(journal.provider.clone()),
                tracking_mode: Some(journal.tracking_mode.clone()),
                tracking_value: journal.tracking_value.clone(),
                current_selected_ref: Some(journal.selected_ref.clone()),
                current_release_id: Some(journal.release_id.clone()),
                aliases: Vec::new(),
                created_at: iso_timestamp(self.clock.unix_epoch_nanos()),
            });
            self.filesystem
                .write_source_transition_journal(library_root, journal)?;
        }
        Ok(())
    }

    fn clean_claim_set(
        &self,
        preview: &SourceGroupPreview,
        require_exact_member_set: bool,
    ) -> Result<CleanClaimSet, SourceTransitionError> {
        let reports = self.lock_store.discover()?;
        let relevant = reports
            .into_iter()
            .filter(|report| {
                report.entries.iter().any(|entry| {
                    parse_git_source_input(&entry.source_url)
                        .map(|spec| spec.url == preview.source_url)
                        .unwrap_or(false)
                })
            })
            .collect::<Vec<_>>();
        if relevant.len() != 1 {
            return Err(SourceTransitionError::Validation(
                "a clean Source Transition requires all claims in exactly one external lock".into(),
            ));
        }
        let report = relevant.into_iter().next().expect("one relevant lock");
        if report.fault.is_some() || !report.entry_faults.is_empty() {
            return Err(SourceTransitionError::Validation(
                "the external lock is not clean enough for a Source Transition".into(),
            ));
        }
        let canonical_root = self.external_skills_root_for_lock(&report.path)?;
        let expected = preview
            .members
            .iter()
            .map(|member| (member.directory_name.clone(), member.skill_path.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut entries = report
            .entries
            .into_iter()
            .filter(|entry| {
                parse_git_source_input(&entry.source_url)
                    .map(|spec| spec.url == preview.source_url)
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        let names = entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<BTreeSet<_>>();
        // A clean transition requires the claims to exactly cover the
        // discovered member set (each member's external entity is what the
        // installer claimed). A Promotion releases whatever the legacy
        // installer claimed for this repository — old member paths may have
        // disappeared upstream (removed) or be absent locally: the single
        // CAS still must release every claim as one exact file update.
        if require_exact_member_set {
            if entries.len() != expected.len()
                || names.len() != expected.len()
                || names != expected.keys().cloned().collect()
            {
                return Err(SourceTransitionError::Validation(
                    "the external lock does not claim the complete source member set".into(),
                ));
            }
            for entry in &entries {
                if expected.get(&entry.name) != Some(&entry.skill_path) {
                    return Err(SourceTransitionError::Validation(format!(
                        "external lock entry '{}' does not match the fixed source release",
                        entry.name
                    )));
                }
            }
        }
        let canonical_entities = entries
            .iter()
            .map(|entry| (entry.name.clone(), canonical_root.join(&entry.name)))
            .collect();
        Ok(CleanClaimSet {
            lock_path: report.path,
            lock_fingerprint: report.fingerprint,
            lock_entries: entries,
            canonical_entities,
        })
    }

    fn lock_claim_state(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<LockClaimState, SourceTransitionError> {
        self.frozen_lock_claim_state(journal, true)
    }

    fn undo_lock_claim_state(
        &self,
        journal: &SourceTransitionJournal,
    ) -> Result<LockClaimState, SourceTransitionError> {
        // A successful conditional restore reserializes the lock, so its
        // full-file fingerprint need not equal the pre-transition bytes on a
        // later crash. Exact complete claims still prove that the one Undo
        // CAS already happened; all other paths retain fingerprint checking.
        self.frozen_lock_claim_state(journal, false)
    }

    fn frozen_lock_claim_state(
        &self,
        journal: &SourceTransitionJournal,
        require_original_fingerprint_when_present: bool,
    ) -> Result<LockClaimState, SourceTransitionError> {
        let reports = self.lock_store.discover()?;
        let report = reports
            .into_iter()
            .find(|report| report.path == journal.lock_path)
            .ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(
                    "the external lock vanished while deciding Source Transition recovery".into(),
                )
            })?;
        if report.fault.is_some() || !report.entry_faults.is_empty() {
            return Err(SourceTransitionError::RecoveryRequired(
                "the external lock cannot be strictly interpreted for Source Transition recovery"
                    .into(),
            ));
        }

        let expected = journal
            .lock_entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let present = report
            .entries
            .iter()
            .filter(|entry| expected.contains_key(entry.name.as_str()))
            .collect::<Vec<_>>();
        if present.len() == expected.len() {
            let all_exact = present.iter().all(|entry| {
                expected
                    .get(entry.name.as_str())
                    .is_some_and(|frozen| *frozen == *entry)
            });
            if !all_exact {
                return Err(SourceTransitionError::RecoveryRequired(
                    "a frozen external source claim changed after the journal was written".into(),
                ));
            }
            if require_original_fingerprint_when_present
                && report.fingerprint != journal.lock_fingerprint
            {
                return Err(SourceTransitionError::RecoveryRequired(
                    "the external lock changed after the journal was written".into(),
                ));
            }
            return Ok(LockClaimState::Present);
        }
        if !present.is_empty() {
            return Err(SourceTransitionError::RecoveryRequired(
                "the external lock contains only part of the frozen source claim set".into(),
            ));
        }
        for entry in &report.entries {
            let source = parse_git_source_input(&entry.source_url).map_err(|_| {
                SourceTransitionError::RecoveryRequired(
                    "the external lock contains an unparsable source while deciding recovery"
                        .into(),
                )
            })?;
            if source.url == journal.canonical_url {
                return Err(SourceTransitionError::RecoveryRequired(
                    "an external claim for the frozen repository reappeared under a different key"
                        .into(),
                ));
            }
        }
        Ok(LockClaimState::Released)
    }

    fn external_skills_root_for_lock(
        &self,
        lock_path: &Path,
    ) -> Result<PathBuf, SourceTransitionError> {
        let default_lock = self.home_directory.join(".agents/.skill-lock.json");
        if lock_path == default_lock {
            return Ok(self.home_directory.join(".agents/skills"));
        }
        if lock_path
            .file_name()
            .is_some_and(|name| name == ".skill-lock.json")
            && lock_path
                .parent()
                .and_then(|parent| parent.file_name())
                .is_some_and(|name| name == "skills")
        {
            return Ok(lock_path
                .parent()
                .expect("skills parent verified")
                .to_path_buf());
        }
        Err(SourceTransitionError::Validation(
            "this external lock has no supported canonical Skills root for Source Transition"
                .into(),
        ))
    }

    fn validate_journal_layout(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        if journal.update_previous.is_some() {
            return self.validate_update_journal_layout(library_root, journal);
        }
        let staging_root = library_root.join("staging").join(&journal.operation_id);
        if journal.staging_operation_root != staging_root {
            return Err(SourceTransitionError::RecoveryRequired(
                "the Source Transition journal staging root is outside its operation".into(),
            ));
        }
        let external_root = self
            .external_skills_root_for_lock(&journal.lock_path)
            .map_err(|_| {
                SourceTransitionError::RecoveryRequired(
                    "the Source Transition journal lock has no supported external Skills root"
                        .into(),
                )
            })?;
        let claims = journal
            .lock_entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        if claims.len() != journal.members.len() {
            return Err(SourceTransitionError::RecoveryRequired(
                "the Source Transition journal does not carry one claim per member".into(),
            ));
        }
        for member in &journal.members {
            if !is_safe_source_transition_member_name(&member.directory_name)
                || !is_safe_source_transition_member_name(&member.skill_id)
                || member.identity_key != skill_identity_key(&member.directory_name)
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal member '{}' has an unsafe identity",
                    member.directory_name
                )));
            }
            let claim = claims.get(member.directory_name.as_str()).cloned();
            let expected_canonical = external_root.join(&member.directory_name);
            if member.staged_root != staging_root.join(&member.directory_name)
                || member.namespace_path
                    != git_member_namespace_path(library_root, &journal.remote_id, &member.skill_id)
                || match (&member.canonical_entity, &claim) {
                    (Some(canonical_entity), Some(_)) => canonical_entity != &expected_canonical,
                    // A member without an external declaration may not claim
                    // one; a claimed member must carry its canonical entity.
                    (Some(_), None) | (None, Some(_)) => true,
                    (None, None) => false,
                }
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal member '{}' escapes its owned path",
                    member.directory_name
                )));
            }
            let expected_isolated = external_root.join(format!(
                ".skill-man-source-transition-{}-{}",
                journal.operation_id, member.directory_name
            ));
            let expected_isolated = self
                .filesystem
                .normalize_configured_path(&expected_isolated)?;
            if let Some(isolated_path) = &member.isolated_path
                && self.filesystem.normalize_configured_path(isolated_path)? != expected_isolated
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal isolation path for '{}' escapes its source root (expected '{}', got '{}')",
                    member.directory_name,
                    expected_isolated.display(),
                    isolated_path.display(),
                )));
            }
            if let Some(claim) = &claim {
                let source = parse_git_source_input(&claim.source_url).map_err(|_| {
                    SourceTransitionError::RecoveryRequired(format!(
                        "the Source Transition journal claim '{}' has an invalid source",
                        claim.name
                    ))
                })?;
                if claim.skill_path != member.skill_path || source.url != journal.canonical_url {
                    return Err(SourceTransitionError::RecoveryRequired(format!(
                        "the Source Transition journal claim '{}' changed repository facts",
                        claim.name
                    )));
                }
            }
        }
        for removed in &journal.removed_members {
            // Removed legacy entities live in the external Skills root (the
            // legacy installer's tree), never under the managed library.
            if !is_safe_source_transition_member_name(&removed.directory_name)
                || !is_safe_source_transition_member_name(&removed.skill_id)
                || !removed.legacy_path.starts_with(&external_root)
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal removed member '{}' escapes its Legacy root",
                    removed.directory_name
                )));
            }
            let expected_isolated = removed
                .legacy_path
                .parent()
                .ok_or_else(|| {
                    SourceTransitionError::RecoveryRequired(
                        "removed member path has no parent".into(),
                    )
                })?
                .join(format!(
                    ".skill-man-source-transition-{}-{}",
                    journal.operation_id, removed.directory_name
                ));
            let expected_isolated = self
                .filesystem
                .normalize_configured_path(&expected_isolated)?;
            if let Some(isolated_path) = &removed.isolated_path
                && self.filesystem.normalize_configured_path(isolated_path)? != expected_isolated
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal removal isolation path for '{}' escapes its root",
                    removed.directory_name
                )));
            }
        }
        Ok(())
    }

    fn validate_update_journal_layout(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        let staging_root = library_root.join("staging").join(&journal.operation_id);
        if journal.staging_operation_root != staging_root {
            return Err(SourceTransitionError::RecoveryRequired(
                "the Source Update journal staging root is outside its operation".into(),
            ));
        }
        for member in &journal.members {
            if !is_safe_source_transition_member_name(&member.directory_name)
                || !is_safe_source_transition_member_name(&member.skill_id)
                || member.identity_key != skill_identity_key(&member.directory_name)
                || member.canonical_entity.is_some()
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Update journal member '{}' has an unsafe identity",
                    member.directory_name
                )));
            }
            if member.staged_root != staging_root.join(&member.directory_name)
                || member.namespace_path
                    != git_member_namespace_path(library_root, &journal.remote_id, &member.skill_id)
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Update journal member '{}' escapes its owned path",
                    member.directory_name
                )));
            }
            let expected_isolated = member
                .namespace_path
                .parent()
                .ok_or_else(|| {
                    SourceTransitionError::RecoveryRequired(
                        "the Update member namespace has no parent".into(),
                    )
                })?
                .join(format!(
                    ".skill-man-source-transition-{}-{}",
                    journal.operation_id, member.skill_id
                ));
            let expected_isolated = self
                .filesystem
                .normalize_configured_path(&expected_isolated)?;
            if let Some(isolated_path) = &member.isolated_path
                && self.filesystem.normalize_configured_path(isolated_path)? != expected_isolated
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Update isolation path for '{}' escapes its namespace",
                    member.directory_name
                )));
            }
        }
        for removed in &journal.removed_members {
            if !is_safe_source_transition_member_name(&removed.directory_name)
                || !is_safe_source_transition_member_name(&removed.skill_id)
                || removed.legacy_path
                    != git_member_namespace_path(
                        library_root,
                        &journal.remote_id,
                        &removed.skill_id,
                    )
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Update removed member '{}' escapes its namespace",
                    removed.directory_name
                )));
            }
            let expected_isolated = removed
                .legacy_path
                .parent()
                .ok_or_else(|| {
                    SourceTransitionError::RecoveryRequired(
                        "removed member path has no parent".into(),
                    )
                })?
                .join(format!(
                    ".skill-man-source-transition-{}-{}",
                    journal.operation_id, removed.skill_id
                ));
            let expected_isolated = self
                .filesystem
                .normalize_configured_path(&expected_isolated)?;
            if let Some(isolated_path) = &removed.isolated_path
                && self.filesystem.normalize_configured_path(isolated_path)? != expected_isolated
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Update removal isolation path for '{}' escapes its namespace",
                    removed.directory_name
                )));
            }
        }
        Ok(())
    }

    fn journal_for(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<SourceTransitionJournal, SourceTransitionError> {
        self.filesystem
            .list_source_transition_journals(library_root)?
            .into_iter()
            .find(|journal| journal.operation_id == operation_id)
            .ok_or_else(|| {
                SourceTransitionError::Validation(
                    "the Source Transition result window is no longer available".into(),
                )
            })
    }

    fn discard_transition_staging(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        self.filesystem.discard_staging(
            &journal.staging_operation_root,
            library_root,
            journal.staging_fingerprint.as_ref(),
        )?;
        Ok(())
    }

    fn active_library_root(&self) -> Result<PathBuf, SourceTransitionError> {
        match &self.home_context {
            Some(context) => context
                .bound_home()
                .map(|home| home.path)
                .map_err(|error| SourceTransitionError::RecoveryRequired(error.to_string())),
            None => Ok(self.configured_library_root.clone()),
        }
    }

    fn capture_write_context(&self) -> Result<HomeWriteContext, SourceTransitionError> {
        self.write_gate
            .capture_open_context()
            .map_err(|error| match error {
                WriteGateError::Stale => SourceTransitionError::PreviewStale,
                WriteGateError::Closed => SourceTransitionError::RecoveryRequired(
                    "startup recovery is still in progress".into(),
                ),
                other => SourceTransitionError::RecoveryRequired(other.to_string()),
            })
    }

    fn acquire_write_guard(
        &self,
        context: &HomeWriteContext,
    ) -> Result<ProductWriteGuard<'_>, SourceTransitionError> {
        self.write_gate
            .acquire_product_write(context)
            .map_err(|error| match error {
                WriteGateError::Stale => SourceTransitionError::PreviewStale,
                WriteGateError::Closed => SourceTransitionError::RecoveryRequired(
                    "startup recovery is still in progress".into(),
                ),
                other => SourceTransitionError::RecoveryRequired(other.to_string()),
            })
    }

    fn library_root_for_context(
        &self,
        context: &HomeWriteContext,
    ) -> Result<PathBuf, SourceTransitionError> {
        if self.home_context.is_some() {
            Ok(context.home.path.clone())
        } else {
            self.active_library_root()
        }
    }

    fn next_operation_id(&self) -> String {
        let number = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!(
            "source-transition-{}-{number}",
            self.clock.unix_epoch_nanos()
        )
    }

    fn next_uuid(&self) -> String {
        let mut bytes = [0_u8; 16];
        if self.filesystem.read_entropy(&mut bytes).is_err() {
            let value =
                self.clock.unix_epoch_nanos() as u64 ^ self.next_id.fetch_add(1, Ordering::Relaxed);
            bytes[..8].copy_from_slice(&value.to_be_bytes());
            bytes[8..].copy_from_slice(&(value.rotate_left(17)).to_be_bytes());
        }
        uuid_v4_shape(&mut bytes)
    }

    fn persist_transition_intent(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        self.filesystem
            .write_source_transition_journal(library_root, journal)
            .map_err(|error| self.block_for_recovery("persist Source Transition intent", error))
    }

    fn block_for_recovery(
        &self,
        context: &str,
        error: impl std::fmt::Display,
    ) -> SourceTransitionError {
        self.write_gate.mark_blocked();
        SourceTransitionError::RecoveryRequired(format!("{context}: {error}"))
    }

    /// Install one staged snapshot into the immutable namespace. `expect` is
    /// the frozen staged snapshot (tree hash + file/byte counts) so a
    /// concurrent change after staging is a pre-CAS stale error.
    fn install_git_snapshot(
        &self,
        staged_root: &Path,
        namespace_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected: &crate::seams::filesystem::StagedTreeSnapshot,
    ) -> Result<(), SourceTransitionError> {
        self.filesystem.install_git_member_snapshot(
            staged_root,
            namespace_path,
            library_root,
            operation_id,
            expected,
        )?;
        Ok(())
    }
}

/// The immutable snapshot path of one Git member (ADR-0018 §Home 与 Catalog).
fn git_member_namespace_path(library_root: &Path, remote_id: &str, skill_id: &str) -> PathBuf {
    library_root
        .join(GIT_SKILLS_NAMESPACE)
        .join(remote_id)
        .join(skill_id)
}

fn is_safe_source_transition_member_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LockClaimState {
    Present,
    Released,
}

/// Fail-closed default: promotion needs a real Legacy reader.
struct UnavailableSourcePromotionStore;

/// Fail-closed default: the Source Update needs a real lifecycle store.
struct UnavailableSourceUpdateStore;

impl SourceUpdateStore for UnavailableSourceUpdateStore {
    fn read_current(
        &self,
        _remote_id: &str,
    ) -> Result<Option<SourceUpdateCurrentSource>, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn source_ids(&self) -> Result<Vec<String>, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn member_health(
        &self,
        _skill_id: &SkillId,
    ) -> Result<Option<(String, crate::core::domain::Health)>, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn validate_source_update(
        &self,
        _record: &SourceUpdateRecord,
    ) -> Result<(), SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn commit_source_update(
        &self,
        _record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn source_update_is_committed(
        &self,
        _record: &SourceUpdateRecord,
    ) -> Result<bool, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn undo_source_update(
        &self,
        _record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn set_source_member_health(
        &self,
        _remote_id: &str,
        _health: &[(SkillId, crate::core::domain::Health)],
    ) -> Result<u64, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn register_local_copy(
        &self,
        _record: &crate::seams::source_update_store::LocalSourceCopyRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn local_copy_is_registered(
        &self,
        _destination: &std::path::Path,
    ) -> Result<bool, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn source_remove_facts(
        &self,
        _remote_id: &str,
    ) -> Result<crate::seams::source_update_store::SourceRemoveFacts, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn commit_remove_source(&self, _remote_id: &str) -> Result<u64, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }

    fn source_remove_is_committed(&self, _remote_id: &str) -> Result<bool, SourceUpdateStoreError> {
        Err(SourceUpdateStoreError::Unavailable(
            "no Source Update store is configured".into(),
        ))
    }
}

impl SourcePromotionStore for UnavailableSourcePromotionStore {
    fn read_legacy_source_promotion(
        &self,
        _remote_id: &str,
    ) -> Result<LegacySourcePromotionRecord, SourcePromotionStoreError> {
        Err(SourcePromotionStoreError::Unavailable(
            "no Legacy Source Promotion store is configured".into(),
        ))
    }

    fn validate_source_promotion(
        &self,
        _record: &SourcePromotionRecord,
    ) -> Result<(), SourcePromotionStoreError> {
        Err(SourcePromotionStoreError::Unavailable(
            "no Legacy Source Promotion store is configured".into(),
        ))
    }

    fn commit_source_promotion(
        &self,
        _record: SourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError> {
        Err(SourcePromotionStoreError::Unavailable(
            "no Legacy Source Promotion store is configured".into(),
        ))
    }

    fn source_promotion_is_committed(
        &self,
        _record: &SourcePromotionRecord,
    ) -> Result<bool, SourcePromotionStoreError> {
        Err(SourcePromotionStoreError::Unavailable(
            "no Legacy Source Promotion store is configured".into(),
        ))
    }

    fn undo_source_promotion(
        &self,
        _record: &SourcePromotionRecord,
        _legacy: &LegacySourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError> {
        Err(SourcePromotionStoreError::Unavailable(
            "no Legacy Source Promotion store is configured".into(),
        ))
    }
}

fn record_from_journal(
    journal: &SourceTransitionJournal,
) -> Result<SourceTransitionRecord, SourceTransitionError> {
    if journal.members.is_empty()
        || journal
            .members
            .iter()
            .any(|member| member.tree_hash.is_empty() || member.staged_snapshot.is_none())
        || journal
            .removed_members
            .iter()
            .any(|member| member.tree_hash.is_empty())
    {
        return Err(SourceTransitionError::RecoveryRequired(
            "the Source Transition journal is missing frozen staged member facts".into(),
        ));
    }
    match &journal.promotion_legacy {
        None => Ok(SourceTransitionRecord {
            remote_id: journal.remote_id.clone(),
            provider: journal.provider.clone(),
            canonical_url: journal.canonical_url.clone(),
            aliases: Vec::new(),
            tracking_mode: journal.tracking_mode.clone(),
            tracking_value: journal.tracking_value.clone(),
            selection_kind: journal.selection_kind.clone(),
            selected_ref: journal.selected_ref.clone(),
            release_id: journal.release_id.clone(),
            resolved_commit: journal.resolved_commit.clone(),
            members: journal_member_records(journal),
        }),
        Some(legacy) => Ok(SourceTransitionRecord {
            remote_id: journal.remote_id.clone(),
            provider: journal.provider.clone(),
            canonical_url: journal.canonical_url.clone(),
            aliases: legacy.aliases.clone(),
            tracking_mode: journal.tracking_mode.clone(),
            tracking_value: journal.tracking_value.clone(),
            selection_kind: journal.selection_kind.clone(),
            selected_ref: journal.selected_ref.clone(),
            release_id: journal.release_id.clone(),
            resolved_commit: journal.resolved_commit.clone(),
            members: journal_member_records(journal),
        }),
    }
}

fn journal_member_records(journal: &SourceTransitionJournal) -> Vec<SourceTransitionMemberRecord> {
    journal
        .members
        .iter()
        .map(|member| SourceTransitionMemberRecord {
            skill_id: SkillId(member.skill_id.clone()),
            directory_name: member.directory_name.clone(),
            identity_key: member.identity_key.clone(),
            display_name: member.display_name.clone(),
            description: member.description.clone(),
            storage_relpath: format!(
                "{GIT_SKILLS_NAMESPACE}/{}/{}",
                journal.remote_id, member.skill_id
            ),
            skill_path: member.skill_path.clone(),
            tree_hash: member.tree_hash.clone(),
            provider_hash: member.provider_hash.clone(),
        })
        .collect()
}

/// Build the v9 Promotion record from a frozen journal. The complete target
/// release members map to `Legacy`/`New` by journal action; removed legacy
/// members carry their frozen audit entity.
pub(crate) fn promotion_record_from_journal(
    journal: &SourceTransitionJournal,
) -> Result<Option<SourcePromotionRecord>, SourceTransitionError> {
    let Some(legacy) = &journal.promotion_legacy else {
        return Ok(None);
    };
    let record = record_from_journal(journal)?;
    let _ = &record;
    let removed_members = journal
        .removed_members
        .iter()
        .map(|removed| {
            let legacy_entity = legacy
                .members
                .iter()
                .find(|member| member.skill_id.0 == removed.skill_id)
                .cloned()
                .ok_or_else(|| {
                    SourceTransitionError::RecoveryRequired(format!(
                        "the removed member '{}' has no frozen legacy audit",
                        removed.directory_name
                    ))
                })?;
            Ok(SourcePromotionRemovedMemberRecord {
                skill_id: SkillId(removed.skill_id.clone()),
                directory_name: removed.directory_name.clone(),
                legacy_entity,
            })
        })
        .collect::<Result<Vec<_>, SourceTransitionError>>()?;
    let members = journal
        .members
        .iter()
        .map(|member| {
            let legacy_entity = member
                .action
                .eq(&crate::seams::filesystem::SourceTransitionMemberAction::Current)
                .then(|| {
                    legacy
                        .members
                        .iter()
                        .find(|entry| entry.skill_id.0 == member.skill_id)
                        .cloned()
                })
                .flatten();
            SourcePromotionMemberRecord {
                origin: if member
                    .action
                    .eq(&crate::seams::filesystem::SourceTransitionMemberAction::Current)
                {
                    SourcePromotionMemberOrigin::Legacy
                } else {
                    SourcePromotionMemberOrigin::New
                },
                skill_id: SkillId(member.skill_id.clone()),
                directory_name: member.directory_name.clone(),
                identity_key: member.identity_key.clone(),
                display_name: member.display_name.clone(),
                description: member.description.clone(),
                storage_relpath: format!(
                    "{GIT_SKILLS_NAMESPACE}/{}/{}",
                    journal.remote_id, member.skill_id
                ),
                skill_path: member.skill_path.clone(),
                tree_hash: member.tree_hash.clone(),
                provider_hash: member.provider_hash.clone(),
                legacy_entity,
            }
        })
        .collect();
    Ok(Some(SourcePromotionRecord {
        remote_id: journal.remote_id.clone(),
        provider: journal.provider.clone(),
        canonical_url: journal.canonical_url.clone(),
        tracking_mode: journal.tracking_mode.clone(),
        tracking_value: journal.tracking_value.clone(),
        selection_kind: journal.selection_kind.clone(),
        selected_ref: journal.selected_ref.clone(),
        release_id: journal.release_id.clone(),
        resolved_commit: journal.resolved_commit.clone(),
        operation_id: journal.operation_id.clone(),
        legacy: legacy.clone(),
        legacy_member_ids: legacy
            .members
            .iter()
            .map(|member| member.skill_id.clone())
            .collect(),
        members,
        removed_members,
    }))
}

fn presence_text(presence: SourceMemberPresence) -> &'static str {
    match presence {
        SourceMemberPresence::Current => "current",
        SourceMemberPresence::Absent => "absent",
    }
}

fn parse_presence_inner(value: &str) -> SourceMemberPresence {
    match value {
        "absent" => SourceMemberPresence::Absent,
        _ => SourceMemberPresence::Current,
    }
}

fn health_text(health: crate::core::domain::Health) -> &'static str {
    match health {
        crate::core::domain::Health::Healthy => "healthy",
        crate::core::domain::Health::Broken => "broken",
        crate::core::domain::Health::Modified => "modified",
        crate::core::domain::Health::SourceSnapshotMismatch => "source_snapshot_mismatch",
    }
}

fn parse_health_inner(value: &str) -> crate::core::domain::Health {
    match value {
        "broken" => crate::core::domain::Health::Broken,
        "modified" => crate::core::domain::Health::Modified,
        "source_snapshot_mismatch" => crate::core::domain::Health::SourceSnapshotMismatch,
        _ => crate::core::domain::Health::Healthy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_namespace_paths_are_stable_by_remote_and_skill_id() {
        let library = Path::new("/Library/skill-man");
        assert_eq!(
            git_member_namespace_path(library, "remote-1", "skill-2"),
            PathBuf::from("/Library/skill-man/skills/git/remote-1/skill-2")
        );
    }
}
