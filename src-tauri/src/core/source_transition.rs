//! Whole Git Repository Source confirmation, recovery and Source Undo.
//!
//! The preview is deliberately read-only. This service replays that preview
//! at confirmation, freezes one exact release in a durable journal, and only
//! then changes Home, the external installer lock and Catalog. The lock's one
//! full-file CAS is the ownership commit point; no post-CAS path fetches Git.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::core::domain::{SkillId, parse_skill_metadata, skill_identity_key};
use crate::core::git_source::{git_mirror_path, parse_git_source_input, resolve_git_ref};
use crate::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreview, SourceGroupPreviewError,
    SourceGroupPreviewOutcome, SourceGroupPreviewService,
};
use crate::core::write_gate::WriteGate;
use crate::seams::clock::{Clock, iso_timestamp, uuid_v4_shape};
use crate::seams::filesystem::{
    FileSystem, FileSystemError, RemoteParentManifest, SourceTransitionJournal,
    SourceTransitionJournalMember, SourceTransitionPhase,
};
use crate::seams::installer_lock_store::{
    InstallerLockError, InstallerLockStore, LockEntry, LockReleaseError,
};
use crate::seams::source::{GitSource, SourceError};
use crate::seams::source_transition_store::{
    SourceTransitionMemberRecord, SourceTransitionRecord, SourceTransitionStore,
    SourceTransitionStoreError,
};

const SOURCE_TRANSITION_JOURNAL_VERSION: u32 = 1;

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
    pub tracking_ref: String,
    /// The Source Group Draft's resolved commit. Confirmation refuses if a
    /// fresh read-only preview no longer resolves the same immutable release.
    pub expected_resolved_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTransitionResult {
    pub operation_id: String,
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
}

/// Core-only orchestration. The client sends only immutable preview facts;
/// all lock entries, filesystem paths and Catalog records are discovered and
/// frozen here instead of being trusted from a DTO.
pub struct SourceTransitionService {
    preview: Arc<SourceGroupPreviewService>,
    git_source: Arc<dyn GitSource>,
    lock_store: Arc<dyn InstallerLockStore>,
    store: Arc<dyn SourceTransitionStore>,
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
    ) -> Self {
        Self {
            preview,
            git_source,
            lock_store,
            store,
            filesystem,
            clock,
            configured_library_root: library_root,
            home_directory,
            home_context: None,
            write_gate: Arc::new(WriteGate::open_for_tests()),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn with_write_gate(mut self, write_gate: Arc<WriteGate>) -> Self {
        self.write_gate = write_gate;
        self
    }

    pub fn with_home_context(mut self, home_context: Arc<WriteGate>) -> Self {
        self.home_context = Some(home_context);
        self
    }

    pub fn confirm(
        &self,
        request: ConfirmSourceTransitionRequest,
    ) -> Result<SourceTransitionResult, SourceTransitionError> {
        self.ensure_writes_ready()?;
        let preview = self.refresh_preview(&request)?;
        if preview.resolved_commit != request.expected_resolved_commit {
            return Err(SourceTransitionError::PreviewStale);
        }
        let library_root = self.active_library_root()?;
        let claims = self.clean_claim_set(&preview)?;
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
            tracking_ref: preview.tracking_ref.clone(),
            resolved_commit: preview.resolved_commit.clone(),
            target_manifest: Some(RemoteParentManifest {
                schema_version: 1,
                remote_id: remote_id.clone(),
                canonical_url: preview.source_url.clone(),
                provider: Some(preview.provider.clone()),
                tracking_mode: None,
                tracking_value: None,
                current_selected_ref: None,
                current_release_id: Some(release_id.clone()),
                aliases: Vec::new(),
                created_at: iso_timestamp(self.clock.unix_epoch_nanos()),
            }),
            lock_path: claims.lock_path,
            lock_fingerprint: claims.lock_fingerprint,
            lock_entries: claims.lock_entries,
            members: preview
                .members
                .iter()
                .map(|member| SourceTransitionJournalMember {
                    skill_id: self.next_uuid(),
                    directory_name: member.directory_name.clone(),
                    identity_key: skill_identity_key(&member.directory_name),
                    display_name: member.display_name.clone(),
                    description: member.description.clone(),
                    canonical_entity: claims
                        .canonical_entities
                        .get(&member.directory_name)
                        .cloned()
                        .expect("clean source claims cover every preview member"),
                    staged_root: library_root
                        .join("staging")
                        .join(&operation_id)
                        .join(&member.directory_name),
                    isolated_path: None,
                    staged_snapshot: None,
                    final_entity_path: library_root.join("skills").join(&member.directory_name),
                    skill_path: member.skill_path.clone(),
                    tree_hash: String::new(),
                    provider_hash: None,
                })
                .collect(),
        };
        self.filesystem
            .write_source_transition_journal(&library_root, &journal)?;

        let result = self.apply_confirmed_transition(&library_root, &preview, &mut journal);
        match result {
            Ok(snapshot_version) => Ok(SourceTransitionResult {
                operation_id,
                release_id,
                resolved_commit: preview.resolved_commit,
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
                self.rollback_pre_commit(&library_root, &mut journal)?;
                self.filesystem
                    .finish_source_transition_journal(&library_root, &journal.operation_id)?;
                Err(error)
            }
            Err(error) => Err(self.block_for_recovery("continue Source Transition", error)),
        }
    }

    /// Startup-only recovery. It reads only the frozen journal: no Fetch,
    /// no remote resolution and no partial source decisions.
    pub fn recover_pending(&self, library_root: &Path) -> Result<(), SourceTransitionError> {
        for mut journal in self
            .filesystem
            .list_source_transition_journals(library_root)?
        {
            self.validate_journal_layout(library_root, &journal)?;
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
        self.ensure_writes_ready()?;
        let library_root = self.active_library_root()?;
        let mut journal = self.journal_for(&library_root, operation_id)?;
        self.validate_journal_layout(&library_root, &journal)?;
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
        self.ensure_writes_ready()?;
        let library_root = self.active_library_root()?;
        let journal = self.journal_for(&library_root, operation_id)?;
        self.validate_journal_layout(&library_root, &journal)?;
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
        self.discard_transition_staging(&library_root, &journal)?;
        self.filesystem
            .finish_source_transition_journal(&library_root, operation_id)?;
        Ok(())
    }

    fn refresh_preview(
        &self,
        request: &ConfirmSourceTransitionRequest,
    ) -> Result<SourceGroupPreview, SourceTransitionError> {
        let outcome = self
            .preview
            .fetch_latest_and_manage(FetchLatestAndManageRequest {
                source_type: request.source_type.clone(),
                source_url: request.source_url.clone(),
                tracking_ref: Some(request.tracking_ref.clone()),
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
        preview: &SourceGroupPreview,
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
        let mut spec = parse_git_source_input(&preview.source_url)
            .map_err(|error| SourceTransitionError::Validation(error.to_string()))?;
        spec.requested_ref = Some(preview.tracking_ref.clone());
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
        journal.phase = SourceTransitionPhase::MembersStaged;
        self.filesystem
            .write_source_transition_journal(library_root, journal)?;

        let record = record_from_journal(journal)?;
        self.store.validate_new_source_transition(&record)?;
        self.ensure_external_trees_match(journal)?;
        for index in 0..journal.members.len() {
            {
                let member = &mut journal.members[index];
                member.isolated_path = Some(
                    self.filesystem
                        .isolate_external_source(&member.canonical_entity, &journal.operation_id)?,
                );
            }
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
        let snapshot_version = self.store.commit_source_transition(record)?;
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

    fn roll_forward(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        self.publish_members(library_root, journal)?;
        let record = record_from_journal(journal)?;
        self.store.commit_source_transition(record)?;
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
            if self
                .filesystem
                .path_is_directory(&member.final_entity_path)?
            {
                let final_snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&member.final_entity_path)?;
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
                self.filesystem.install_staged_skill(
                    &member.staged_root,
                    &member.final_entity_path,
                    library_root,
                    &journal.operation_id,
                    expected,
                )?;
            }
        }
        Ok(())
    }

    fn rollback_pre_commit(
        &self,
        library_root: &Path,
        journal: &mut SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        if journal.phase == SourceTransitionPhase::DestinationsReserved {
            for member in journal.members.iter().rev() {
                if !self
                    .filesystem
                    .path_is_occupied(&member.final_entity_path)?
                {
                    continue;
                }
                if !self
                    .filesystem
                    .path_is_directory(&member.final_entity_path)?
                {
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
                    .staged_tree_snapshot(&member.final_entity_path)?;
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
                    .remove_directory_verified(&member.final_entity_path)?;
            }
        }
        for member in journal.members.iter_mut().rev() {
            if let Some(isolated) = &member.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.restore_isolated_source(
                    isolated,
                    &member.canonical_entity,
                    &member.tree_hash,
                )?;
            }
            member.isolated_path = None;
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
        let catalog_is_committed = self.store.source_transition_is_committed(&record)?;
        // Revalidate the entire conditional Undo after the durable Undoing
        // cursor exists and immediately before Catalog mutation. A changed
        // Home path, external path, lock or preservation copy is never
        // silently converted into a partial Undo.
        if catalog_is_committed {
            self.preflight_undo(journal)?;
        }
        let snapshot_version = if catalog_is_committed {
            self.store.undo_source_transition(&record)?
        } else {
            0
        };
        self.filesystem
            .remove_remote_parent_manifest(&library_root.join("remotes"), &journal.remote_id)?;
        for member in &journal.members {
            if self
                .filesystem
                .path_is_occupied(&member.final_entity_path)?
            {
                if !self
                    .filesystem
                    .path_is_directory(&member.final_entity_path)?
                {
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
                    .staged_tree_snapshot(&member.final_entity_path)?;
                if final_snapshot.content_hash != member.tree_hash {
                    return Err(self.block_for_recovery(
                        "complete Source Undo",
                        format!("Home member '{}' changed", member.directory_name),
                    ));
                }
                self.filesystem
                    .remove_directory_verified(&member.final_entity_path)?;
            }
            let isolated = member.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the Source Undo journal has no external copy for '{}'",
                    member.directory_name
                ))
            })?;
            if self.filesystem.path_is_occupied(&member.canonical_entity)? {
                // A previous recovery may already have restored this member.
                // Only accept that idempotent state when the preserved copy
                // has gone and the canonical tree is exactly frozen; an
                // occupied path while its preservation copy remains is a
                // concurrent external owner and must remain locked.
                if self.filesystem.path_is_directory(isolated)?
                    || !self
                        .filesystem
                        .path_is_directory(&member.canonical_entity)?
                {
                    return Err(self.block_for_recovery(
                        "complete Source Undo",
                        format!(
                            "the external source location for '{}' is occupied",
                            member.directory_name
                        ),
                    ));
                }
                let canonical_snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&member.canonical_entity)?;
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
                    &member.canonical_entity,
                    &member.tree_hash,
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
        if !self.store.source_transition_is_committed(&record)? {
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
            if self.filesystem.path_is_occupied(&member.canonical_entity)? {
                return Err(SourceTransitionError::Validation(format!(
                    "the external source location for '{}' is occupied",
                    member.directory_name
                )));
            }
            if !self
                .filesystem
                .path_is_directory(&member.final_entity_path)?
            {
                return Err(SourceTransitionError::Validation(format!(
                    "the Managed Skill '{}' is no longer a directory",
                    member.directory_name
                )));
            }
            let final_snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.final_entity_path)?;
            if final_snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::Validation(format!(
                    "the Managed Skill '{}' changed after confirmation",
                    member.directory_name
                )));
            }
            let isolated = member.isolated_path.as_ref().ok_or_else(|| {
                SourceTransitionError::Validation(format!(
                    "the external preservation copy for '{}' is unavailable",
                    member.directory_name
                ))
            })?;
            let isolated_snapshot = self.filesystem.staged_tree_snapshot(isolated)?;
            if isolated_snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::Validation(format!(
                    "the external preservation copy for '{}' changed",
                    member.directory_name
                )));
            }
        }
        Ok(())
    }

    fn reserve_member_destinations(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), SourceTransitionError> {
        for member in &journal.members {
            if self
                .filesystem
                .path_is_occupied(&member.final_entity_path)?
            {
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
            self.filesystem.install_staged_skill(
                &member.staged_root,
                &member.final_entity_path,
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
            if self
                .filesystem
                .path_is_occupied(&member.final_entity_path)?
            {
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
            if !self
                .filesystem
                .path_is_directory(&member.final_entity_path)?
            {
                return Err(SourceTransitionError::PreviewStale);
            }
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.final_entity_path)?;
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
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.canonical_entity)?;
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
            if self
                .filesystem
                .path_is_directory(&member.canonical_entity)?
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "external source '{}' reappeared after the ownership commit point",
                    member.directory_name
                )));
            }
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.final_entity_path)?;
            if snapshot.content_hash != member.tree_hash {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "Managed Skill '{}' does not match the fixed Source Release",
                    member.directory_name
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
                tracking_mode: None,
                tracking_value: None,
                current_selected_ref: None,
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
        if entries.len() != expected.len()
            || names.len() != expected.len()
            || names != expected.keys().cloned().collect()
        {
            return Err(SourceTransitionError::Validation(
                "the external lock does not claim the complete source member set".into(),
            ));
        }
        for entry in &entries {
            if expected.get(&entry.name) != Some(&entry.skill_path)
                || entry.requested_ref.as_deref().unwrap_or("HEAD") != preview.tracking_ref
            {
                return Err(SourceTransitionError::Validation(format!(
                    "external lock entry '{}' does not match the fixed source release",
                    entry.name
                )));
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
                || member.identity_key != skill_identity_key(&member.directory_name)
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal member '{}' has an unsafe identity",
                    member.directory_name
                )));
            }
            let Some(claim) = claims.get(member.directory_name.as_str()) else {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal member '{}' has no frozen lock claim",
                    member.directory_name
                )));
            };
            if member.staged_root != staging_root.join(&member.directory_name)
                || member.final_entity_path
                    != library_root.join("skills").join(&member.directory_name)
                || member.canonical_entity != external_root.join(&member.directory_name)
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
            let source = parse_git_source_input(&claim.source_url).map_err(|_| {
                SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal claim '{}' has an invalid source",
                    claim.name
                ))
            })?;
            if claim.skill_path != member.skill_path
                || source.url != journal.canonical_url
                || claim.requested_ref.as_deref().unwrap_or("HEAD") != journal.tracking_ref
            {
                return Err(SourceTransitionError::RecoveryRequired(format!(
                    "the Source Transition journal claim '{}' changed repository facts",
                    claim.name
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

    fn ensure_writes_ready(&self) -> Result<(), SourceTransitionError> {
        if self.write_gate.is_product_write_open() {
            Ok(())
        } else {
            Err(SourceTransitionError::RecoveryRequired(
                "startup recovery is still in progress".into(),
            ))
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

    fn block_for_recovery(
        &self,
        context: &str,
        error: impl std::fmt::Display,
    ) -> SourceTransitionError {
        self.write_gate.mark_blocked();
        SourceTransitionError::RecoveryRequired(format!("{context}: {error}"))
    }
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

fn record_from_journal(
    journal: &SourceTransitionJournal,
) -> Result<SourceTransitionRecord, SourceTransitionError> {
    if journal.members.is_empty()
        || journal
            .members
            .iter()
            .any(|member| member.tree_hash.is_empty() || member.staged_snapshot.is_none())
    {
        return Err(SourceTransitionError::RecoveryRequired(
            "the Source Transition journal is missing frozen staged member facts".into(),
        ));
    }
    Ok(SourceTransitionRecord {
        remote_id: journal.remote_id.clone(),
        provider: journal.provider.clone(),
        canonical_url: journal.canonical_url.clone(),
        tracking_ref: journal.tracking_ref.clone(),
        release_id: journal.release_id.clone(),
        resolved_commit: journal.resolved_commit.clone(),
        members: journal
            .members
            .iter()
            .map(|member| SourceTransitionMemberRecord {
                skill_id: SkillId(member.skill_id.clone()),
                directory_name: member.directory_name.clone(),
                identity_key: member.identity_key.clone(),
                display_name: member.display_name.clone(),
                description: member.description.clone(),
                library_entry_path: member.final_entity_path.clone(),
                final_entity_path: member.final_entity_path.clone(),
                skill_path: member.skill_path.clone(),
                tree_hash: member.tree_hash.clone(),
                provider_hash: member.provider_hash.clone(),
            })
            .collect(),
    })
}
