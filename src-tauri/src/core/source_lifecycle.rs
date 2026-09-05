//! v9 Git Source lifecycle operations (ticket #93): Restore Current Source
//! Release, Create Local Source Copy and whole-source Remove. Each uses its
//! own journaled phases; recovery reads the frozen journals only.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::core::domain::{Health, SkillId, skill_identity_key};
use crate::core::git_source::{git_mirror_path, parse_git_source_input, resolve_git_ref};
use crate::core::source_transition::SourceTransitionService;
use crate::core::write_gate::{HomeWriteContext, ProductWriteGuard, WriteGate, WriteGateError};
use crate::seams::clock::{Clock, uuid_v4_shape};
use crate::seams::filesystem::{
    ActivationEntrySnapshot, DirectoryFingerprint, FileSystem, FileSystemError, LocalCopyJournal,
    LocalCopyPhase, RemoveSourceActivationJournal, RemoveSourceJournal, RemoveSourceMemberJournal,
    RemoveSourcePhase, RestoreSourceJournal, RestoreSourceMember, RestoreSourcePhase,
    SourceLifecycleJournal, StagedTreeSnapshot,
};
use crate::seams::source::{GitSource, SourceError};
use crate::seams::source_update_store::{
    LocalSourceCopyRecord, SourceMemberPresence, SourceUpdateCurrentMember,
    SourceUpdateCurrentSource, SourceUpdateStoreError,
};

const LIFECYCLE_JOURNAL_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRestoreResult {
    pub remote_id: String,
    pub restored_members: u32,
    pub snapshot_version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocalCopyResult {
    pub operation_id: String,
    pub skill_id: SkillId,
    pub directory_name: String,
    pub destination: PathBuf,
    pub snapshot_version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRemoveResult {
    pub operation_id: String,
    pub remote_id: String,
    pub member_count: u32,
    pub snapshot_version: u64,
}

#[derive(Debug, Error)]
pub enum SourceLifecycleError {
    #[error("{0}")]
    Validation(String),
    #[error(
        "the Git Source Member snapshots do not match the current Source Release; Restore Current Source Release before continuing"
    )]
    SourceSnapshotMismatch,
    #[error("the lifecycle operation requires recovery: {0}")]
    RecoveryRequired(String),
    #[error(transparent)]
    Store(#[from] SourceUpdateStoreError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error(transparent)]
    Source(#[from] SourceError),
}

pub struct SourceLifecycleService {
    transition: Arc<SourceTransitionService>,
    filesystem: Arc<dyn FileSystem>,
    git_source: Arc<dyn GitSource>,
    clock: Arc<dyn Clock>,
    configured_library_root: PathBuf,
    home_context: Option<Arc<WriteGate>>,
    app_state_dir: Option<PathBuf>,
    write_gate: Arc<WriteGate>,
    next_id: AtomicU64,
}

impl SourceLifecycleService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        transition: Arc<SourceTransitionService>,
        filesystem: Arc<dyn FileSystem>,
        git_source: Arc<dyn GitSource>,
        clock: Arc<dyn Clock>,
        library_root: PathBuf,
    ) -> Self {
        let write_gate = transition.write_gate();
        Self {
            transition,
            filesystem,
            git_source,
            clock,
            configured_library_root: library_root,
            home_context: None,
            app_state_dir: None,
            write_gate,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn with_home_context(mut self) -> Self {
        self.home_context = Some(self.write_gate.clone());
        self
    }

    /// The App-level state directory (`~/Library/Application Support/
    /// skill-man-state`): a Local Source Copy must never land there
    /// (spec §8.3, ticket #93 criterion 6).
    pub fn with_app_state_dir(mut self, app_state_dir: PathBuf) -> Self {
        self.app_state_dir = Some(app_state_dir);
        self
    }

    pub fn restore_current_release(
        &self,
        remote_id: &str,
    ) -> Result<SourceRestoreResult, SourceLifecycleError> {
        let write_context = self.capture_write_context()?;
        let library_root = self.library_root_for_context(&write_context)?;
        let _write_guard = self.acquire_write_guard(&write_context)?;
        let current = self
            .transition
            .update_store_handle()
            .read_current(remote_id)?
            .ok_or_else(|| {
                SourceLifecycleError::Validation(
                    "the selected Git Repository Source is no longer complete".into(),
                )
            })?;
        let mut members_to_restore = Vec::new();
        for member in current
            .members
            .iter()
            .filter(|member| member.presence == SourceMemberPresence::Current)
        {
            let namespace = library_root.join(&member.storage_relpath);
            let observed = match self.observed_hash(&namespace) {
                Ok(hash) => Some(hash),
                Err(SourceLifecycleError::FileSystem(FileSystemError::Io { source, .. }))
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    None
                }
                Err(error) => return Err(error),
            };
            if observed.as_deref() != member.tree_hash.as_deref() {
                members_to_restore.push((member.clone(), observed));
            }
        }
        let operation_id = self.next_operation_id("restore");
        let staging_operation_root = library_root.join("staging").join(&operation_id);
        let journal = RestoreSourceJournal {
            version: LIFECYCLE_JOURNAL_VERSION,
            operation_id: operation_id.clone(),
            phase: RestoreSourcePhase::Planned,
            remote_id: remote_id.into(),
            release_id: current.current_release_id.clone(),
            resolved_commit: current.resolved_commit.clone(),
            staging_operation_root: staging_operation_root.clone(),
            staging_fingerprint: None,
            members: members_to_restore
                .iter()
                .map(|(member, observed)| RestoreSourceMember {
                    skill_id: member.skill_id.0.clone(),
                    directory_name: member.directory_name.clone(),
                    skill_path: member.skill_path.clone(),
                    namespace_path: library_root.join(&member.storage_relpath),
                    staged_root: staging_operation_root.join(&member.directory_name),
                    release_tree_hash: member.tree_hash.clone().unwrap_or_else(|| "missing".into()),
                    observed_tree_hash: observed.clone().unwrap_or_default(),
                    staged_snapshot: None,
                    backup_path: None,
                    restored: false,
                })
                .collect(),
        };
        self.persist_lifecycle_intent(
            &library_root,
            &SourceLifecycleJournal::Restore(journal.clone()),
        )?;
        if journal.members.is_empty() {
            if let Err(error) = self
                .filesystem
                .finish_source_lifecycle_journal(&library_root, &operation_id)
            {
                return Err(self.block_for_recovery("archive empty Source Restore journal", error));
            }
            let snapshot_version = self
                .transition
                .update_store_handle()
                .read_current(remote_id)?
                .map(|_| 0)
                .unwrap_or(0);
            return Ok(SourceRestoreResult {
                remote_id: remote_id.into(),
                restored_members: 0,
                snapshot_version,
            });
        }
        let mut journal = journal;

        let result = self.apply_restore(&library_root, &mut journal);
        match result {
            Ok(snapshot_version) => {
                journal.phase = RestoreSourcePhase::Finalized;
                if let Err(error) = self.filesystem.write_source_lifecycle_journal(
                    &library_root,
                    &SourceLifecycleJournal::Restore(journal.clone()),
                ) {
                    return Err(
                        self.block_for_recovery("persist finalized Source Restore journal", error)
                    );
                }
                if let Err(error) = self.cleanup_restore(&library_root, &journal) {
                    return Err(self.block_for_recovery("clean up completed Source Restore", error));
                }
                Ok(SourceRestoreResult {
                    remote_id: remote_id.into(),
                    restored_members: u32::try_from(journal.members.len()).map_err(|_| {
                        SourceLifecycleError::Validation("too many source members".into())
                    })?,
                    snapshot_version,
                })
            }
            Err(error)
                if journal.phase != RestoreSourcePhase::Restored
                    && journal.phase != RestoreSourcePhase::Committed =>
            {
                if let Err(compensation) = self.rollback_restore(&library_root, &journal) {
                    return Err(
                        self.block_for_recovery("roll back failed Source Restore", compensation)
                    );
                }
                if let Err(archive) = self
                    .filesystem
                    .finish_source_lifecycle_journal(&library_root, &operation_id)
                {
                    return Err(
                        self.block_for_recovery("archive failed Source Restore journal", archive)
                    );
                }
                Err(error)
            }
            Err(error) => {
                Err(self.block_for_recovery("continue Restore Current Source Release", error))
            }
        }
    }

    /// Create Local Source Copy: copy the current observed member bytes
    /// (Healthy or mismatched) to a user-chosen directory outside Home,
    /// Agent/installer roots and App state, then register the canonical
    /// final entity path as a Local Source. No `.git`, no content baseline;
    /// the source, member and Activations stay unchanged.
    pub fn create_local_copy(
        &self,
        remote_id: &str,
        skill_id: &str,
        destination: &Path,
    ) -> Result<SourceLocalCopyResult, SourceLifecycleError> {
        let write_context = self.capture_write_context()?;
        let library_root = self.library_root_for_context(&write_context)?;
        let _write_guard = self.acquire_write_guard(&write_context)?;
        let current = self
            .transition
            .update_store_handle()
            .read_current(remote_id)?
            .ok_or_else(|| {
                SourceLifecycleError::Validation(
                    "the selected Git Repository Source is no longer complete".into(),
                )
            })?;
        let member = current
            .members
            .iter()
            .find(|member| member.skill_id.0 == skill_id)
            .cloned()
            .ok_or_else(|| {
                SourceLifecycleError::Validation(
                    "the selected Git Source Member is not part of this source".into(),
                )
            })?;
        if member.presence != SourceMemberPresence::Current {
            return Err(SourceLifecycleError::Validation(
                "the Git Source Member is tombstoned; there are no bytes to copy".into(),
            ));
        }
        let source_path = library_root.join(&member.storage_relpath);
        let destination_parent =
            self.validate_local_copy_destination(destination, &source_path, &current)?;
        let destination_name = destination
            .file_name()
            .ok_or_else(|| {
                SourceLifecycleError::Validation(
                    "the Local Source Copy destination has no directory name".into(),
                )
            })?
            .to_owned();
        // Persist the canonical final path, not an alias whose parent could
        // later resolve through a different filesystem component.
        let destination = destination_parent.canonical_path.join(destination_name);
        let operation_id = self.next_operation_id("local-copy");
        let staged_path = destination
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .join(format!(
                ".skill-man-source-transition-{operation_id}-{}",
                member.directory_name
            ));
        let content_hash = self
            .filesystem
            .staged_tree_snapshot(&source_path)?
            .content_hash;
        let mut journal = LocalCopyJournal {
            version: LIFECYCLE_JOURNAL_VERSION,
            operation_id: operation_id.clone(),
            phase: LocalCopyPhase::Planned,
            remote_id: remote_id.into(),
            skill_id: skill_id.into(),
            directory_name: member.directory_name.clone(),
            identity_key: member.identity_key.clone(),
            display_name: member.display_name.clone(),
            description: member.description.clone(),
            source_path: source_path.clone(),
            destination: destination.clone(),
            destination_parent: Some(destination_parent),
            destination_fingerprint: None,
            staged_path: staged_path.clone(),
            content_hash,
        };
        self.persist_lifecycle_intent(
            &library_root,
            &SourceLifecycleJournal::LocalCopy(journal.clone()),
        )?;

        let result = self.apply_local_copy(&library_root, &mut journal);
        match result {
            Ok((snapshot_version, local_skill_id, local_directory_name)) => {
                if let Err(error) = self
                    .filesystem
                    .finish_source_lifecycle_journal(&library_root, &operation_id)
                {
                    return Err(self
                        .block_for_recovery("archive completed Local Source Copy journal", error));
                }
                Ok(SourceLocalCopyResult {
                    operation_id,
                    skill_id: SkillId(local_skill_id),
                    directory_name: local_directory_name,
                    destination: destination.to_path_buf(),
                    snapshot_version,
                })
            }
            Err(error) => {
                if let Err(compensation) = self.rollback_local_copy(&library_root, &journal) {
                    return Err(
                        self.block_for_recovery("roll back failed Local Source Copy", compensation)
                    );
                }
                if let Err(archive) = self
                    .filesystem
                    .finish_source_lifecycle_journal(&library_root, &operation_id)
                {
                    return Err(self
                        .block_for_recovery("archive failed Local Source Copy journal", archive));
                }
                Err(error)
            }
        }
    }

    /// Ordinary whole-source Remove: the source is the only Remove unit.
    /// Member snapshots are isolated for rollback, Activation entries are
    /// removed after target verification, then one Catalog transaction
    /// deletes the source, its releases, members, tombstones and skill rows.
    pub fn remove_source(
        &self,
        remote_id: &str,
    ) -> Result<SourceRemoveResult, SourceLifecycleError> {
        let write_context = self.capture_write_context()?;
        let library_root = self.library_root_for_context(&write_context)?;
        let _write_guard = self.acquire_write_guard(&write_context)?;
        let current = self
            .transition
            .update_store_handle()
            .read_current(remote_id)?
            .ok_or_else(|| {
                SourceLifecycleError::Validation(
                    "the selected Git Repository Source is no longer complete".into(),
                )
            })?;
        // Spec §8.2: a Snapshot Mismatch blocks ordinary source writes;
        // only read-only view, Disable, Create Local Source Copy and
        // explicit Restore stay available. Remove is not exempt.
        for member in current
            .members
            .iter()
            .filter(|member| member.presence == SourceMemberPresence::Current)
        {
            let namespace = library_root.join(&member.storage_relpath);
            let observed = match self.observed_hash(&namespace) {
                Ok(hash) => hash,
                Err(SourceLifecycleError::FileSystem(FileSystemError::Io { source, .. }))
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    String::new()
                }
                Err(error) => return Err(error),
            };
            if observed != member.tree_hash.clone().unwrap_or_default() {
                return Err(SourceLifecycleError::SourceSnapshotMismatch);
            }
            if member.health == Health::SourceSnapshotMismatch {
                return Err(SourceLifecycleError::SourceSnapshotMismatch);
            }
        }
        let facts = self
            .transition
            .update_store_handle()
            .source_remove_facts(remote_id)?;
        let operation_id = self.next_operation_id("remove");
        let staging_operation_root = library_root.join("staging").join(&operation_id);
        let journal = RemoveSourceJournal {
            version: LIFECYCLE_JOURNAL_VERSION,
            operation_id: operation_id.clone(),
            phase: RemoveSourcePhase::Planned,
            remote_id: remote_id.into(),
            canonical_url: current.canonical_url.clone(),
            staging_operation_root,
            staging_fingerprint: None,
            members: current
                .members
                .iter()
                .map(|member| RemoveSourceMemberJournal {
                    skill_id: member.skill_id.0.clone(),
                    directory_name: member.directory_name.clone(),
                    namespace_path: library_root.join(&member.storage_relpath),
                    observed_tree_hash: member.tree_hash.clone().unwrap_or_default(),
                    isolated_path: None,
                })
                .collect(),
            activations: facts
                .activations
                .iter()
                .map(|activation| RemoveSourceActivationJournal {
                    skill_id: activation.skill_id.0.clone(),
                    entry_path: activation.entry_path.clone(),
                    target_path: activation.target_path.clone(),
                })
                .collect(),
        };
        self.persist_lifecycle_intent(
            &library_root,
            &SourceLifecycleJournal::RemoveSource(journal.clone()),
        )?;
        let mut journal = journal;
        let result = self.apply_remove_source(&library_root, &mut journal);
        match result {
            Ok(snapshot_version) => {
                if let Err(error) = self.cleanup_remove_source(&library_root, &journal) {
                    return Err(self.block_for_recovery("clean up completed Source Remove", error));
                }
                if let Err(error) = self
                    .filesystem
                    .finish_source_lifecycle_journal(&library_root, &operation_id)
                {
                    return Err(
                        self.block_for_recovery("archive completed Source Remove journal", error)
                    );
                }
                Ok(SourceRemoveResult {
                    operation_id,
                    remote_id: remote_id.into(),
                    member_count: u32::try_from(current.members.len()).map_err(|_| {
                        SourceLifecycleError::Validation("too many source members".into())
                    })?,
                    snapshot_version,
                })
            }
            Err(error) if journal.phase != RemoveSourcePhase::CatalogCommitted => {
                if let Err(compensation) = self.rollback_remove_source(&library_root, &journal) {
                    return Err(
                        self.block_for_recovery("roll back failed Source Remove", compensation)
                    );
                }
                if let Err(archive) = self
                    .filesystem
                    .finish_source_lifecycle_journal(&library_root, &operation_id)
                {
                    return Err(
                        self.block_for_recovery("archive failed Source Remove journal", archive)
                    );
                }
                Err(error)
            }
            Err(error) => Err(self.block_for_recovery("continue whole-source Remove", error)),
        }
    }

    /// Startup recovery for Restore / Local Copy / Remove journals. It
    /// reads only the frozen journals; no remote is ever fetched again.
    pub fn recover_lifecycle(&self, library_root: &Path) -> Result<(), SourceLifecycleError> {
        for journal in self
            .filesystem
            .list_source_lifecycle_journals(library_root)?
        {
            self.validate_recovery_journal(library_root, &journal)?;
            match &journal {
                SourceLifecycleJournal::Restore(restore)
                    if matches!(
                        restore.phase,
                        RestoreSourcePhase::Restored | RestoreSourcePhase::Committed
                    ) =>
                {
                    self.finish_restore_after_crash(library_root, restore)?;
                }
                SourceLifecycleJournal::Restore(restore) => {
                    self.rollback_restore(library_root, restore)?;
                    self.filesystem
                        .finish_source_lifecycle_journal(library_root, &restore.operation_id)?;
                }
                SourceLifecycleJournal::LocalCopy(local)
                    if matches!(local.phase, LocalCopyPhase::Registered) =>
                {
                    self.finish_local_copy_after_crash(library_root, local)?;
                }
                SourceLifecycleJournal::LocalCopy(local) => {
                    // The catalog registration never happened (or the rename
                    // did not): rolling back restores a clean state and
                    // closes the journal instead of blocking forever.
                    self.rollback_local_copy(library_root, local)?;
                    self.filesystem
                        .finish_source_lifecycle_journal(library_root, &local.operation_id)?;
                }
                SourceLifecycleJournal::RemoveSource(remove)
                    if matches!(
                        remove.phase,
                        RemoveSourcePhase::CatalogCommitted | RemoveSourcePhase::Finalized
                    ) =>
                {
                    self.roll_forward_remove_source(library_root, remove)?;
                }
                SourceLifecycleJournal::RemoveSource(remove) => {
                    self.rollback_remove_source(library_root, remove)?;
                    self.filesystem
                        .finish_source_lifecycle_journal(library_root, &remove.operation_id)?;
                }
            }
        }
        Ok(())
    }

    fn validate_recovery_journal(
        &self,
        library_root: &Path,
        journal: &SourceLifecycleJournal,
    ) -> Result<(), SourceLifecycleError> {
        let operation_id = journal.operation_id();
        if !(operation_id.starts_with("source-transition-restore-")
            || operation_id.starts_with("source-transition-local-copy-")
            || operation_id.starts_with("source-transition-remove-"))
            || !is_safe_path_component(operation_id)
        {
            return Err(Self::invalid_recovery_journal(
                "the operation id is not a safe Source Lifecycle identity",
            ));
        }
        let staging_root = library_root.join("staging").join(operation_id);
        match journal {
            SourceLifecycleJournal::Restore(restore) => {
                if restore.staging_operation_root != staging_root {
                    return Err(Self::invalid_recovery_journal(
                        "the Restore staging root is not operation-scoped",
                    ));
                }
                let current = self.current_recovery_source(&restore.remote_id)?;
                for member in &restore.members {
                    let current_member = Self::current_recovery_member(&current, &member.skill_id)?;
                    if current_member.presence != SourceMemberPresence::Current
                        || member.skill_path != current_member.skill_path
                        || member.release_tree_hash
                            != current_member.tree_hash.as_deref().unwrap_or_default()
                        || member.namespace_path
                            != library_root.join(&current_member.storage_relpath)
                        || !is_safe_path_component(&member.directory_name)
                        || member.staged_root != staging_root.join(&member.directory_name)
                    {
                        return Err(Self::invalid_recovery_journal(
                            "the Restore journal does not match the current Source member",
                        ));
                    }
                    if let Some(backup) = &member.backup_path {
                        let normalized_namespace = self
                            .filesystem
                            .normalize_configured_path(&member.namespace_path)?;
                        let expected = isolated_source_path(&normalized_namespace, operation_id)?;
                        if backup != &expected {
                            return Err(Self::invalid_recovery_journal(
                                "the Restore isolation path is not source-scoped",
                            ));
                        }
                    }
                }
            }
            SourceLifecycleJournal::LocalCopy(local) => {
                let current = self.current_recovery_source(&local.remote_id)?;
                let current_member = Self::current_recovery_member(&current, &local.skill_id)?;
                let local_copy_registered = self.local_copy_is_registered(&local.destination)?;
                if current_member.presence != SourceMemberPresence::Current
                    || local.directory_name != current_member.directory_name
                    || local.identity_key != current_member.identity_key
                    || local.source_path != library_root.join(&current_member.storage_relpath)
                    || !is_safe_path_component(&local.directory_name)
                    || !is_safe_destination_path(&local.destination)
                    || local.staged_path
                        != local
                            .destination
                            .parent()
                            .unwrap_or_else(|| Path::new("/"))
                            .join(format!(
                                ".skill-man-source-transition-{operation_id}-{}",
                                local.directory_name
                            ))
                        && !matches!(local.phase, LocalCopyPhase::Registered)
                    || (!local_copy_registered
                        && (local.display_name != current_member.display_name
                            || local.description != current_member.description))
                    || self
                        .filesystem
                        .staged_tree_snapshot(&local.source_path)
                        .map(|snapshot| snapshot.content_hash != local.content_hash)
                        .unwrap_or(true)
                {
                    return Err(Self::invalid_recovery_journal(
                        "the Local Copy journal does not match the current Source member",
                    ));
                }
                if !local_copy_registered {
                    self.validate_local_copy_destination(
                        &local.destination,
                        &local.source_path,
                        &current,
                    )
                    .map(|_| ())
                    .map_err(|error| {
                        Self::invalid_recovery_journal(&format!(
                            "the Local Copy destination is unsafe: {error}"
                        ))
                    })?;
                } else {
                    let parent = local.destination.parent().ok_or_else(|| {
                        Self::invalid_recovery_journal("the Local Copy destination has no parent")
                    })?;
                    let expected_parent = local.destination_parent.as_ref().ok_or_else(|| {
                        Self::invalid_recovery_journal(
                            "the Local Copy journal has no destination parent identity",
                        )
                    })?;
                    let expected_destination =
                        local.destination_fingerprint.as_ref().ok_or_else(|| {
                            Self::invalid_recovery_journal(
                                "the Local Copy journal has no destination identity",
                            )
                        })?;
                    if !self
                        .filesystem
                        .path_has_no_symlink_component(&local.destination)
                        .map_err(|error| Self::invalid_recovery_journal(&error.to_string()))?
                        || self
                            .filesystem
                            .directory_fingerprint(parent)
                            .map_err(|error| Self::invalid_recovery_journal(&error.to_string()))?
                            != *expected_parent
                        || self
                            .filesystem
                            .directory_fingerprint(&local.destination)
                            .map_err(|error| Self::invalid_recovery_journal(&error.to_string()))?
                            != *expected_destination
                    {
                        return Err(Self::invalid_recovery_journal(
                            "the Local Copy destination parent changed",
                        ));
                    }
                }
            }
            SourceLifecycleJournal::RemoveSource(remove) => {
                if remove.staging_operation_root != staging_root {
                    return Err(Self::invalid_recovery_journal(
                        "the Remove staging root is not operation-scoped",
                    ));
                }
                if matches!(
                    remove.phase,
                    RemoveSourcePhase::CatalogCommitted | RemoveSourcePhase::Finalized
                ) {
                    if !is_safe_path_component(&remove.remote_id) {
                        return Err(Self::invalid_recovery_journal(
                            "the Remove Source identity is not path-safe",
                        ));
                    }
                    for member in &remove.members {
                        if !is_safe_path_component(&member.skill_id)
                            || !is_safe_path_component(&member.directory_name)
                        {
                            return Err(Self::invalid_recovery_journal(
                                "the committed Remove member identity is not path-safe",
                            ));
                        }
                        let expected = library_root
                            .join("skills")
                            .join("git")
                            .join(&remove.remote_id)
                            .join(&member.skill_id);
                        if member.namespace_path != expected {
                            return Err(Self::invalid_recovery_journal(
                                "the committed Remove namespace escapes the Git source root",
                            ));
                        }
                        if let Some(isolated) = &member.isolated_path {
                            let normalized_namespace = self
                                .filesystem
                                .normalize_configured_path(&member.namespace_path)?;
                            let expected =
                                isolated_source_path(&normalized_namespace, operation_id)?;
                            if isolated != &expected {
                                return Err(Self::invalid_recovery_journal(
                                    "the committed Remove isolation path is not source-scoped",
                                ));
                            }
                        }
                    }
                    return Ok(());
                }
                let current = self.current_recovery_source(&remove.remote_id)?;
                if remove.canonical_url != current.canonical_url {
                    return Err(Self::invalid_recovery_journal(
                        "the Remove journal Source identity changed",
                    ));
                }
                for member in &remove.members {
                    let current_member = Self::current_recovery_member(&current, &member.skill_id)?;
                    if member.directory_name != current_member.directory_name
                        || member.namespace_path
                            != library_root.join(&current_member.storage_relpath)
                    {
                        return Err(Self::invalid_recovery_journal(
                            "the Remove journal does not match the current Source member",
                        ));
                    }
                    if let Some(isolated) = &member.isolated_path {
                        let normalized_namespace = self
                            .filesystem
                            .normalize_configured_path(&member.namespace_path)?;
                        let expected = isolated_source_path(&normalized_namespace, operation_id)?;
                        if isolated != &expected {
                            return Err(Self::invalid_recovery_journal(
                                "the Remove isolation path is not source-scoped",
                            ));
                        }
                    }
                }
                let facts = self
                    .transition
                    .update_store_handle()
                    .source_remove_facts(&remove.remote_id)?;
                let expected = facts
                    .activations
                    .iter()
                    .map(|activation| {
                        (
                            activation.skill_id.0.clone(),
                            activation.entry_path.clone(),
                            activation.target_path.clone(),
                        )
                    })
                    .collect::<BTreeSet<_>>();
                let actual = remove
                    .activations
                    .iter()
                    .map(|activation| {
                        (
                            activation.skill_id.clone(),
                            activation.entry_path.clone(),
                            activation.target_path.clone(),
                        )
                    })
                    .collect::<BTreeSet<_>>();
                if actual != expected {
                    return Err(Self::invalid_recovery_journal(
                        "the Remove journal Activation set changed",
                    ));
                }
            }
        }
        Ok(())
    }

    fn current_recovery_source(
        &self,
        remote_id: &str,
    ) -> Result<SourceUpdateCurrentSource, SourceLifecycleError> {
        self.transition
            .update_store_handle()
            .read_current(remote_id)?
            .ok_or_else(|| {
                Self::invalid_recovery_journal(
                    "the journal Source no longer exists in the current Catalog",
                )
            })
    }

    fn current_recovery_member<'a>(
        current: &'a SourceUpdateCurrentSource,
        skill_id: &str,
    ) -> Result<&'a SourceUpdateCurrentMember, SourceLifecycleError> {
        current
            .members
            .iter()
            .find(|member| member.skill_id.0 == skill_id)
            .ok_or_else(|| {
                Self::invalid_recovery_journal(
                    "the journal member no longer exists in the current Catalog",
                )
            })
    }

    fn invalid_recovery_journal(message: &str) -> SourceLifecycleError {
        SourceLifecycleError::RecoveryRequired(format!(
            "the Source Lifecycle journal is unsafe: {message}"
        ))
    }

    fn local_copy_is_registered(&self, destination: &Path) -> Result<bool, SourceLifecycleError> {
        if self
            .transition
            .update_store_handle()
            .local_copy_is_registered(destination)?
        {
            return Ok(true);
        }
        let Ok(canonical) = self.filesystem.normalize_configured_path(destination) else {
            return Ok(false);
        };
        if canonical == destination {
            return Ok(false);
        }
        Ok(self
            .transition
            .update_store_handle()
            .local_copy_is_registered(&canonical)?)
    }

    // ---- Restore ---------------------------------------------------------

    fn apply_restore(
        &self,
        library_root: &Path,
        journal: &mut RestoreSourceJournal,
    ) -> Result<u64, SourceLifecycleError> {
        let fingerprint = self
            .filesystem
            .create_adopt_staging_operation(library_root, &journal.operation_id)?;
        journal.staging_fingerprint = Some(fingerprint);
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::Restore(journal.clone()),
        )?;

        // Only the persisted current release: fixed selected ref and
        // resolved commit, never a fresh policy resolution.
        let source = self
            .transition
            .update_store_handle()
            .read_current(&journal.remote_id)?
            .ok_or_else(|| {
                SourceLifecycleError::Validation(
                    "the selected Git Repository Source is no longer complete".into(),
                )
            })?;
        let temporary_mirror_root = tempfile::Builder::new()
            .prefix("skill-man-source-restore-")
            .tempdir()
            .map_err(|source| {
                SourceLifecycleError::Source(SourceError::Io {
                    operation: "create temporary Git restore mirror",
                    path: std::env::temp_dir(),
                    source,
                })
            })?;
        let mut spec = parse_git_source_input(&source.canonical_url)
            .map_err(|error| SourceLifecycleError::Validation(error.to_string()))?;
        spec.requested_ref = Some(source.selected_ref.clone());
        let mirror = git_mirror_path(temporary_mirror_root.path(), &spec.url);
        let report = self.git_source.fetch_mirror(&spec.url, &mirror)?;
        let resolved = resolve_git_ref(self.git_source.as_ref(), &mirror, &spec, &report)
            .map_err(|error| SourceLifecycleError::Validation(error.to_string()))?;
        if resolved.commit != source.resolved_commit {
            return Err(SourceLifecycleError::Validation(
                "the persisted Source Release commit is no longer resolvable; run Update after restoring".into(),
            ));
        }
        for index in 0..journal.members.len() {
            {
                let member = &mut journal.members[index];
                self.git_source.stage_skill(
                    &mirror,
                    &source.resolved_commit,
                    &member.skill_path,
                    &member.staged_root,
                )?;
                let snapshot = self.filesystem.staged_tree_snapshot(&member.staged_root)?;
                crate::core::import::validate_staged_tree(self.filesystem.as_ref(), &snapshot)
                    .map_err(|error| SourceLifecycleError::Validation(error.to_string()))?;
                if snapshot.content_hash != member.release_tree_hash {
                    return Err(SourceLifecycleError::Validation(
                        "the persisted Source Release bytes changed; Restore is refused".into(),
                    ));
                }
                member.staged_snapshot = Some(snapshot.clone());
            }
            self.filesystem.write_source_lifecycle_journal(
                library_root,
                &SourceLifecycleJournal::Restore(journal.clone()),
            )?;
        }
        journal.phase = RestoreSourcePhase::Restaged;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::Restore(journal.clone()),
        )?;

        // Isolate the observed bytes before installing anything.
        for index in 0..journal.members.len() {
            {
                let member = &mut journal.members[index];
                if self.filesystem.path_is_occupied(&member.namespace_path)? {
                    let observed = self
                        .filesystem
                        .staged_tree_snapshot(&member.namespace_path)?;
                    if observed.content_hash != member.observed_tree_hash {
                        return Err(SourceLifecycleError::RecoveryRequired(format!(
                            "the observed member '{}' changed after the Restore plan",
                            member.directory_name
                        )));
                    }
                    member.backup_path =
                        Some(self.filesystem.isolate_external_source(
                            &member.namespace_path,
                            &journal.operation_id,
                        )?);
                }
            }
            self.filesystem.write_source_lifecycle_journal(
                library_root,
                &SourceLifecycleJournal::Restore(journal.clone()),
            )?;
        }
        journal.phase = RestoreSourcePhase::BackedUp;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::Restore(journal.clone()),
        )?;

        for index in 0..journal.members.len() {
            {
                let member = &mut journal.members[index];
                let expected = member.staged_snapshot.clone().ok_or_else(|| {
                    SourceLifecycleError::RecoveryRequired(
                        "the Restore journal has no staged snapshot".into(),
                    )
                })?;
                self.install_restored_snapshot(
                    &member.staged_root,
                    &member.namespace_path,
                    library_root,
                    &journal.operation_id,
                    &expected,
                )?;
                member.restored = true;
            }
            self.filesystem.write_source_lifecycle_journal(
                library_root,
                &SourceLifecycleJournal::Restore(journal.clone()),
            )?;
        }
        journal.phase = RestoreSourcePhase::Restored;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::Restore(journal.clone()),
        )?;

        // Catalog commit point: health rows back to Healthy.
        let health = journal
            .members
            .iter()
            .map(|member| (SkillId(member.skill_id.clone()), Health::Healthy))
            .collect::<Vec<_>>();
        let snapshot_version = self
            .transition
            .update_store_handle()
            .set_source_member_health(&journal.remote_id, &health)?;
        journal.phase = RestoreSourcePhase::Committed;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::Restore(journal.clone()),
        )?;
        Ok(snapshot_version)
    }

    fn cleanup_restore(
        &self,
        library_root: &Path,
        journal: &RestoreSourceJournal,
    ) -> Result<(), SourceLifecycleError> {
        for member in &journal.members {
            if let Some(backup) = &member.backup_path
                && self.filesystem.path_is_directory(backup)?
            {
                self.filesystem.discard_isolated_source(backup)?;
            }
        }
        self.filesystem.discard_staging(
            &journal.staging_operation_root,
            library_root,
            journal.staging_fingerprint.as_ref(),
        )?;
        Ok(())
    }

    fn rollback_restore(
        &self,
        library_root: &Path,
        journal: &RestoreSourceJournal,
    ) -> Result<(), SourceLifecycleError> {
        for member in &journal.members {
            if let Some(backup) = &member.backup_path {
                if self.filesystem.path_is_occupied(&member.namespace_path)? {
                    let installed = self
                        .filesystem
                        .staged_tree_snapshot(&member.namespace_path)?;
                    if installed.content_hash != member.release_tree_hash {
                        return Err(self.block_for_recovery(
                            "roll back Restore Current Source Release",
                            format!(
                                "the restored member '{}' changed during rollback",
                                member.directory_name
                            ),
                        ));
                    }
                    self.filesystem.remove_directory_verified_nofollow(
                        &member.namespace_path,
                        &installed.root,
                    )?;
                }
                self.filesystem.restore_isolated_source(
                    backup,
                    &member.namespace_path,
                    &member.observed_tree_hash,
                )?;
            }
        }
        self.filesystem.discard_staging(
            &journal.staging_operation_root,
            library_root,
            journal.staging_fingerprint.as_ref(),
        )?;
        Ok(())
    }

    /// Post-crash convergence: install missing restored bytes and clean up.
    fn finish_restore_after_crash(
        &self,
        library_root: &Path,
        journal: &RestoreSourceJournal,
    ) -> Result<(), SourceLifecycleError> {
        for member in &journal.members {
            if !self.filesystem.path_is_directory(&member.namespace_path)? {
                return Err(self.block_for_recovery(
                    "finish Restore Current Source Release",
                    format!("the restored member '{}' is missing", member.directory_name),
                ));
            }
            let snapshot = self
                .filesystem
                .staged_tree_snapshot(&member.namespace_path)?;
            if snapshot.content_hash != member.release_tree_hash {
                return Err(self.block_for_recovery(
                    "finish Restore Current Source Release",
                    format!(
                        "the restored member '{}' does not match the release",
                        member.directory_name
                    ),
                ));
            }
            if let Some(backup) = &member.backup_path
                && self.filesystem.path_is_directory(backup)?
            {
                self.filesystem.discard_isolated_source(backup)?;
            }
        }
        // The byte-level commit point passed (the journal was already at
        // Restored/Committed), so the catalog health commit that follows
        // the install must roll forward too; otherwise the member stays
        // `source_snapshot_mismatch` until the next startup verification.
        let health = journal
            .members
            .iter()
            .map(|member| (SkillId(member.skill_id.clone()), Health::Healthy))
            .collect::<Vec<_>>();
        self.transition
            .update_store_handle()
            .set_source_member_health(&journal.remote_id, &health)?;
        self.filesystem.discard_staging(
            &journal.staging_operation_root,
            library_root,
            journal.staging_fingerprint.as_ref(),
        )?;
        self.filesystem
            .finish_source_lifecycle_journal(library_root, &journal.operation_id)?;
        Ok(())
    }

    fn observed_hash(&self, namespace: &Path) -> Result<String, SourceLifecycleError> {
        Ok(self
            .filesystem
            .staged_tree_snapshot(namespace)?
            .content_hash)
    }

    fn install_restored_snapshot(
        &self,
        staged_root: &Path,
        namespace_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected: &StagedTreeSnapshot,
    ) -> Result<(), SourceLifecycleError> {
        self.filesystem.install_git_member_snapshot(
            staged_root,
            namespace_path,
            library_root,
            operation_id,
            expected,
        )?;
        Ok(())
    }

    // ---- Local Copy ------------------------------------------------------

    fn validate_local_copy_destination(
        &self,
        destination: &Path,
        source_path: &Path,
        current: &SourceUpdateCurrentSource,
    ) -> Result<DirectoryFingerprint, SourceLifecycleError> {
        if !destination.is_absolute() {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy destination must be an absolute path".into(),
            ));
        }
        if !self.filesystem.path_has_no_symlink_component(destination)? {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy destination must not contain symlink components".into(),
            ));
        }
        let parent = destination.parent().ok_or_else(|| {
            SourceLifecycleError::Validation("the Local Copy destination has no parent".into())
        })?;
        if !self.filesystem.path_has_no_symlink_component(parent)? {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy parent must not contain symlink components".into(),
            ));
        }
        if !self.filesystem.path_is_occupied(parent)? {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy parent directory must exist".into(),
            ));
        }
        if matches!(
            self.filesystem.activation_snapshot(source_path)?,
            ActivationEntrySnapshot::Symlink { .. }
        ) {
            return Err(SourceLifecycleError::Validation(
                "the Git Source Member snapshot is a symlink".into(),
            ));
        }
        let parent_identity = self.filesystem.directory_fingerprint(parent)?;
        let canonical_destination =
            parent_identity
                .canonical_path
                .join(destination.file_name().ok_or_else(|| {
                    SourceLifecycleError::Validation(
                        "the Local Copy destination has no directory name".into(),
                    )
                })?);
        let source_identity = self.filesystem.directory_fingerprint(source_path)?;
        if canonical_destination == source_identity.canonical_path
            || canonical_destination.starts_with(&source_identity.canonical_path)
        {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy cannot be the source member itself".into(),
            ));
        }
        let library_root = self.active_library_root()?;
        let library_root = self.filesystem.directory_fingerprint(&library_root)?;
        if canonical_destination.starts_with(&library_root.canonical_path) {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy destination must be outside the Skill Man Home".into(),
            ));
        }
        if let Some(app_state_dir) = &self.app_state_dir
            && canonical_destination
                .starts_with(&self.filesystem.normalize_configured_path(app_state_dir)?)
        {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy destination must be outside the App state directory".into(),
            ));
        }
        for forbidden in &current.forbidden_local_link_roots {
            let forbidden = self.filesystem.normalize_configured_path(forbidden)?;
            if canonical_destination.starts_with(&forbidden) {
                return Err(SourceLifecycleError::Validation(format!(
                    "the Local Source Copy destination must be outside '{}'",
                    forbidden.display()
                )));
            }
        }
        if !self.filesystem.path_is_writable(parent)? {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy parent directory is not writable".into(),
            ));
        }
        if self.filesystem.path_is_occupied(destination)?
            && (!self.filesystem.path_is_directory(destination)?
                || !self.filesystem.list_directory(destination)?.is_empty())
        {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy destination must be absent or empty".into(),
            ));
        }
        Ok(parent_identity)
    }

    fn apply_local_copy(
        &self,
        library_root: &Path,
        journal: &mut LocalCopyJournal,
    ) -> Result<(u64, String, String), SourceLifecycleError> {
        let destination_parent = journal.destination_parent.as_ref().ok_or_else(|| {
            SourceLifecycleError::RecoveryRequired(
                "the Local Source Copy journal has no destination parent identity".into(),
            )
        })?;
        let current_parent =
            self.filesystem
                .directory_fingerprint(journal.destination.parent().ok_or_else(|| {
                    SourceLifecycleError::Validation(
                        "the Local Source Copy destination has no parent".into(),
                    )
                })?)?;
        if &current_parent != destination_parent {
            return Err(SourceLifecycleError::Validation(
                "the Local Source Copy destination parent changed after planning".into(),
            ));
        }
        self.filesystem.copy_tree_verified_nofollow(
            &journal.source_path,
            &journal.staged_path,
            destination_parent,
        )?;
        // The staged copy must still equal the snapshot bytes.
        let staged_snapshot = self.filesystem.staged_tree_snapshot(&journal.staged_path)?;
        if staged_snapshot.content_hash != journal.content_hash {
            return Err(SourceLifecycleError::Validation(
                "the staged Local Source Copy no longer matches the member bytes".into(),
            ));
        }
        journal.phase = LocalCopyPhase::Copied;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::LocalCopy(journal.clone()),
        )?;
        let staged_snapshot = self.filesystem.staged_tree_snapshot(&journal.staged_path)?;
        self.filesystem.move_directory_nofollow(
            &journal.staged_path,
            &journal.destination,
            &staged_snapshot,
            destination_parent,
            destination_parent,
        )?;
        journal.destination_fingerprint = Some(
            self.filesystem
                .directory_fingerprint(&journal.destination)?,
        );
        journal.phase = LocalCopyPhase::Registered;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::LocalCopy(journal.clone()),
        )?;
        // The Local Source is a NEW Library entity: it keeps the member's
        // display facts but owns a fresh stable id and the identity of the
        // directory the user actually chose, so Git member and copies (and
        // copies with different folder names) coexist.
        let directory_name = journal
            .destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| {
                SourceLifecycleError::Validation(
                    "the Local Source Copy destination has no directory name".into(),
                )
            })?;
        let local_skill_id = self.next_uuid();
        let snapshot_version =
            self.transition
                .update_store_handle()
                .register_local_copy(&LocalSourceCopyRecord {
                    skill_id: SkillId(local_skill_id.clone()),
                    directory_name: directory_name.clone(),
                    identity_key: skill_identity_key(&directory_name),
                    display_name: journal.display_name.clone(),
                    description: journal.description.clone(),
                    final_entity_path: journal.destination.clone(),
                })?;
        Ok((snapshot_version, local_skill_id, directory_name))
    }

    fn rollback_local_copy(
        &self,
        _library_root: &Path,
        journal: &LocalCopyJournal,
    ) -> Result<(), SourceLifecycleError> {
        if self.filesystem.path_is_directory(&journal.staged_path)? {
            self.filesystem
                .discard_isolated_source(&journal.staged_path)?;
        }
        if self.filesystem.path_is_occupied(&journal.destination)? {
            let expected_destination =
                journal.destination_fingerprint.as_ref().ok_or_else(|| {
                    self.block_for_recovery(
                        "roll back Create Local Source Copy",
                        "the copied destination has no frozen identity".to_string(),
                    )
                })?;
            let actual_destination = self
                .filesystem
                .directory_fingerprint(&journal.destination)
                .map_err(|error| {
                    self.block_for_recovery(
                        "roll back Create Local Source Copy",
                        format!("the copied destination identity is unavailable: {error}"),
                    )
                })?;
            if &actual_destination != expected_destination {
                return Err(self.block_for_recovery(
                    "roll back Create Local Source Copy",
                    "the copied destination identity changed during rollback".to_string(),
                ));
            }
            let destination_snapshot =
                self.filesystem.staged_tree_snapshot(&journal.destination)?;
            if destination_snapshot.content_hash != journal.content_hash {
                return Err(self.block_for_recovery(
                    "roll back Create Local Source Copy",
                    "the copied destination changed during rollback".to_string(),
                ));
            }
            self.filesystem
                .remove_directory_verified_nofollow(&journal.destination, expected_destination)?;
        }
        Ok(())
    }

    fn finish_local_copy_after_crash(
        &self,
        _library_root: &Path,
        journal: &LocalCopyJournal,
    ) -> Result<(), SourceLifecycleError> {
        if !self.filesystem.path_is_directory(&journal.destination)? {
            return Err(self.block_for_recovery(
                "finish Create Local Source Copy",
                format!(
                    "the copied Local Source '{}' is missing",
                    journal.directory_name
                ),
            ));
        }
        // The rename passed but the catalog registration may not have: a
        // Local Source that has no row is not a registered Local Source.
        if !self.local_copy_is_registered(&journal.destination)? {
            let directory_name = journal
                .destination
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| {
                    SourceLifecycleError::Validation(
                        "the Local Source Copy destination has no directory name".into(),
                    )
                })?;
            self.transition.update_store_handle().register_local_copy(
                &crate::seams::source_update_store::LocalSourceCopyRecord {
                    skill_id: SkillId(self.next_uuid()),
                    directory_name,
                    identity_key: skill_identity_key(
                        &journal
                            .destination
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy(),
                    ),
                    display_name: journal.display_name.clone(),
                    description: journal.description.clone(),
                    final_entity_path: journal.destination.clone(),
                },
            )?;
        }
        self.filesystem
            .finish_source_lifecycle_journal(_library_root, &journal.operation_id)?;
        Ok(())
    }

    // ---- Source Remove ---------------------------------------------------

    fn apply_remove_source(
        &self,
        library_root: &Path,
        journal: &mut RemoveSourceJournal,
    ) -> Result<u64, SourceLifecycleError> {
        let fingerprint = self
            .filesystem
            .create_adopt_staging_operation(library_root, &journal.operation_id)?;
        journal.staging_fingerprint = Some(fingerprint);
        for index in 0..journal.members.len() {
            {
                let member = &mut journal.members[index];
                if self.filesystem.path_is_occupied(&member.namespace_path)? {
                    if !self.filesystem.path_is_directory(&member.namespace_path)? {
                        return Err(SourceLifecycleError::RecoveryRequired(format!(
                            "the member '{}' is not a directory",
                            member.directory_name
                        )));
                    }
                    member.isolated_path =
                        Some(self.filesystem.isolate_external_source(
                            &member.namespace_path,
                            &journal.operation_id,
                        )?);
                }
            }
            self.filesystem.write_source_lifecycle_journal(
                library_root,
                &SourceLifecycleJournal::RemoveSource(journal.clone()),
            )?;
        }
        journal.phase = RemoveSourcePhase::MembersIsolated;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::RemoveSource(journal.clone()),
        )?;
        for activation in &journal.activations {
            match self
                .filesystem
                .activation_snapshot(&activation.entry_path)?
            {
                ActivationEntrySnapshot::Symlink { target } if target == activation.target_path => {
                    self.filesystem.remove_activation(&activation.entry_path)?;
                }
                ActivationEntrySnapshot::Missing => {}
                _ => {
                    return Err(SourceLifecycleError::Validation(format!(
                        "the Activation entry '{}' is occupied; resolve it before removing the source",
                        activation.entry_path.display()
                    )));
                }
            }
        }
        journal.phase = RemoveSourcePhase::ActivationsRemoved;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::RemoveSource(journal.clone()),
        )?;
        let snapshot_version = self
            .transition
            .update_store_handle()
            .commit_remove_source(&journal.remote_id)?;
        journal.phase = RemoveSourcePhase::CatalogCommitted;
        self.filesystem.write_source_lifecycle_journal(
            library_root,
            &SourceLifecycleJournal::RemoveSource(journal.clone()),
        )?;
        Ok(snapshot_version)
    }

    fn cleanup_remove_source(
        &self,
        _library_root: &Path,
        journal: &RemoveSourceJournal,
    ) -> Result<(), SourceLifecycleError> {
        for member in &journal.members {
            if let Some(isolated) = &member.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.discard_isolated_source(isolated)?;
            }
        }
        Ok(())
    }

    fn rollback_remove_source(
        &self,
        library_root: &Path,
        journal: &RemoveSourceJournal,
    ) -> Result<(), SourceLifecycleError> {
        for member in journal.members.iter().rev() {
            if let Some(isolated) = &member.isolated_path {
                self.filesystem.restore_isolated_source(
                    isolated,
                    &member.namespace_path,
                    &member.observed_tree_hash,
                )?;
            }
        }
        for activation in journal.activations.iter().rev() {
            match self
                .filesystem
                .activation_snapshot(&activation.entry_path)?
            {
                ActivationEntrySnapshot::Missing => {
                    self.filesystem
                        .create_activation(&activation.target_path, &activation.entry_path)?;
                }
                ActivationEntrySnapshot::Symlink { target } if target == activation.target_path => {
                }
                _ => {
                    return Err(self.block_for_recovery(
                        "roll back whole-source Remove",
                        format!(
                            "the Activation entry '{}' is occupied",
                            activation.entry_path.display()
                        ),
                    ));
                }
            }
        }
        self.filesystem.discard_staging(
            &journal.staging_operation_root,
            library_root,
            journal.staging_fingerprint.as_ref(),
        )?;
        Ok(())
    }

    fn roll_forward_remove_source(
        &self,
        _library_root: &Path,
        journal: &RemoveSourceJournal,
    ) -> Result<(), SourceLifecycleError> {
        if !self
            .transition
            .update_store_handle()
            .source_remove_is_committed(&journal.remote_id)?
        {
            self.transition
                .update_store_handle()
                .commit_remove_source(&journal.remote_id)?;
        }
        for member in &journal.members {
            if let Some(isolated) = &member.isolated_path
                && self.filesystem.path_is_directory(isolated)?
            {
                self.filesystem.discard_isolated_source(isolated)?;
            }
        }
        self.filesystem
            .finish_source_lifecycle_journal(_library_root, &journal.operation_id)?;
        Ok(())
    }

    fn capture_write_context(&self) -> Result<HomeWriteContext, SourceLifecycleError> {
        self.write_gate.capture_open_context().map_err(|_| {
            SourceLifecycleError::RecoveryRequired("startup recovery is still in progress".into())
        })
    }

    fn acquire_write_guard(
        &self,
        context: &HomeWriteContext,
    ) -> Result<ProductWriteGuard<'_>, SourceLifecycleError> {
        self.write_gate
            .acquire_product_write(context)
            .map_err(|error| match error {
                WriteGateError::Stale => SourceLifecycleError::RecoveryRequired(
                    "the Bound Home changed before the operation could commit".into(),
                ),
                WriteGateError::Closed => SourceLifecycleError::RecoveryRequired(
                    "startup recovery is still in progress".into(),
                ),
                other => SourceLifecycleError::RecoveryRequired(other.to_string()),
            })
    }

    fn library_root_for_context(
        &self,
        context: &HomeWriteContext,
    ) -> Result<PathBuf, SourceLifecycleError> {
        if self.home_context.is_some() {
            Ok(context.home.path.clone())
        } else {
            self.active_library_root()
        }
    }

    fn active_library_root(&self) -> Result<PathBuf, SourceLifecycleError> {
        match &self.home_context {
            Some(context) => context
                .bound_home()
                .map(|home| home.path)
                .map_err(|error| SourceLifecycleError::RecoveryRequired(error.to_string())),
            None => Ok(self.configured_library_root.clone()),
        }
    }

    fn persist_lifecycle_intent(
        &self,
        library_root: &Path,
        journal: &SourceLifecycleJournal,
    ) -> Result<(), SourceLifecycleError> {
        self.filesystem
            .write_source_lifecycle_journal(library_root, journal)
            .map_err(|error| self.block_for_recovery("persist Source Lifecycle intent", error))
    }

    fn block_for_recovery(
        &self,
        context: &str,
        error: impl std::fmt::Display,
    ) -> SourceLifecycleError {
        self.write_gate.mark_blocked();
        SourceLifecycleError::RecoveryRequired(format!("{context}: {error}"))
    }

    fn next_operation_id(&self, kind: &str) -> String {
        let number = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!(
            "source-transition-{kind}-{}-{number}",
            self.clock.unix_epoch_nanos()
        )
    }

    /// A fresh stable id for a newly registered entity (a Local Source).
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
}

fn is_safe_path_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn is_safe_destination_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
}

fn isolated_source_path(
    namespace: &Path,
    operation_id: &str,
) -> Result<PathBuf, SourceLifecycleError> {
    let parent = namespace.parent().ok_or_else(|| {
        SourceLifecycleService::invalid_recovery_journal("the source namespace has no parent")
    })?;
    let name = namespace
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| is_safe_path_component(name))
        .ok_or_else(|| {
            SourceLifecycleService::invalid_recovery_journal(
                "the source namespace has no safe member name",
            )
        })?;
    Ok(parent.join(format!(
        ".skill-man-source-transition-{operation_id}-{name}"
    )))
}
