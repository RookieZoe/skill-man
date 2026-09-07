//! v9 Git Repository Source Update (ticket #93, spec §8.3–§8.4,
//! ADR-0018): the read-only Update draft, the whole-source Update confirm
//! through the immutable Source Transition (same journal/CAS/recovery/Undo
//! machinery as a fresh transition) and the member snapshot verifier that
//! persists `source_snapshot_mismatch` and gates Update and new Enable.
//!
//! Restore Current Source Release, Create Local Source Copy and whole-source
//! Remove live in `source_lifecycle.rs` with their own journals.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::core::domain::{Health, SkillId};
use crate::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPolicyFacts, SourceGroupPreviewError,
    SourceGroupPreviewOutcome, SourceGroupPreviewService, SourceTrackingOverride,
};
use crate::core::source_transition::{
    ConfirmSourceUpdateRequest, SourceTransitionError, SourceTransitionResult,
    SourceTransitionService, SourceUndoResult,
};
use crate::core::write_gate::WriteGate;
use crate::seams::filesystem::{FileSystem, FileSystemError};
use crate::seams::source::SourceError;
use crate::seams::source_update_store::{SourceMemberPresence, SourceUpdateStoreError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceUpdateMemberState {
    /// Already current in the managed source (same `skill_path`), including
    /// members that reappear after a tombstone: the `skill_id` and storage
    /// path are reused.
    Current,
    /// A member of the discovered release without a current counterpart.
    Added,
    /// A current member absent from the discovered release.
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateDraftMember {
    pub plugin_name: Option<String>,
    pub skill_id: String,
    pub skill_path: String,
    pub directory_name: String,
    pub directory_identity_key: String,
    pub display_name: String,
    pub description: String,
    pub tree_summary: String,
    pub state: SourceUpdateMemberState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateDraft {
    pub remote_id: String,
    pub provider: String,
    pub source_url: String,
    pub aliases: Vec<String>,
    pub policy: SourceGroupPolicyFacts,
    /// The complete classified manifest of one fetch against the managed
    /// current release.
    pub members: Vec<SourceUpdateDraftMember>,
}

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
pub enum SourceUpdateError {
    #[error("{0}")]
    Validation(String),
    #[error(
        "the Git Source Member snapshots do not match the current Source Release; Restore Current Source Release before continuing"
    )]
    SourceSnapshotMismatch,
    #[error("the restore requires recovery: {0}")]
    RecoveryRequired(String),
    #[error(transparent)]
    Preview(#[from] SourceGroupPreviewError),
    #[error(transparent)]
    Transition(SourceTransitionError),
    #[error(transparent)]
    Store(#[from] SourceUpdateStoreError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// The source lifecycle service. The client sends only durable ids;
/// every member list, release and path fact is re-read by Core/seams.
pub struct SourceUpdateService {
    preview: Arc<SourceGroupPreviewService>,
    transition: Arc<SourceTransitionService>,
    filesystem: Arc<dyn FileSystem>,
    configured_library_root: PathBuf,
    home_context: Option<Arc<WriteGate>>,
    write_gate: Arc<WriteGate>,
}

impl SourceUpdateService {
    pub fn new(
        preview: Arc<SourceGroupPreviewService>,
        transition: Arc<SourceTransitionService>,
        filesystem: Arc<dyn FileSystem>,
        library_root: PathBuf,
    ) -> Self {
        let write_gate = transition.write_gate();
        Self {
            preview,
            transition,
            filesystem,
            configured_library_root: library_root,
            home_context: None,
            write_gate,
        }
    }

    pub fn with_home_context(mut self) -> Self {
        self.home_context = Some(self.write_gate.clone());
        self
    }

    /// Read-only classification of a fresh complete release against the
    /// managed source (including tombstoned members).
    pub fn preview(
        &self,
        remote_id: &str,
        tracking_policy: Option<SourceTrackingOverride>,
    ) -> Result<SourceUpdateDraft, SourceUpdateError> {
        let current = self
            .transition
            .update_store_handle()
            .read_current(remote_id)?
            .ok_or_else(|| {
                SourceUpdateError::Validation(
                    "the selected Git Repository Source is no longer complete".into(),
                )
            })?;
        let canonical_url = current.canonical_url.clone();
        let source_type = if canonical_url.starts_with("https://github.com/") {
            "github"
        } else if canonical_url.starts_with("https://gitlab.com/") {
            "gitlab"
        } else {
            "git"
        };
        // An Update without an explicit override re-evaluates the source's own
        // persisted Source Tracking Policy; only the initial Adopt path uses
        // the `auto_release_tag_head` default (spec §8.4).
        let effective_policy = tracking_policy.or_else(|| {
            Some(crate::core::source_group_preview::SourceTrackingOverride {
                mode: current.tracking_mode.clone(),
                value: current.tracking_value.clone(),
            })
        });
        let outcome = self
            .preview
            .fetch_latest_and_manage(FetchLatestAndManageRequest {
                source_type: source_type.into(),
                source_url: canonical_url,
                tracking_policy: effective_policy,
            })?;
        let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
            return Err(SourceUpdateError::Validation(
                "the current source draft is conflicted; resolve it and preview again".into(),
            ));
        };
        let current_by_path = current
            .members
            .iter()
            .map(|member| (member.skill_path.clone(), member))
            .collect::<BTreeMap<_, _>>();
        let mut members = preview
            .members
            .iter()
            .map(|member| {
                let state = if current_by_path.contains_key(&member.skill_path) {
                    SourceUpdateMemberState::Current
                } else {
                    SourceUpdateMemberState::Added
                };
                let skill_id = current_by_path
                    .get(&member.skill_path)
                    .map(|member| member.skill_id.0.clone())
                    .unwrap_or_default();
                SourceUpdateDraftMember {
                    plugin_name: member.plugin_name.clone(),
                    skill_id,
                    skill_path: member.skill_path.clone(),
                    directory_name: current_by_path
                        .get(&member.skill_path)
                        .map(|existing| existing.directory_name.clone())
                        .unwrap_or_else(|| member.directory_name.clone()),
                    directory_identity_key: current_by_path
                        .get(&member.skill_path)
                        .map(|existing| existing.identity_key.clone())
                        .unwrap_or_else(|| member.directory_identity_key.clone()),
                    display_name: member.display_name.clone(),
                    description: member.description.clone(),
                    tree_summary: member.tree_summary.clone(),
                    state,
                }
            })
            .collect::<Vec<_>>();
        let discovered_paths = preview
            .members
            .iter()
            .map(|member| member.skill_path.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for (skill_path, member) in &current_by_path {
            if member.presence == SourceMemberPresence::Current
                && !discovered_paths.contains(skill_path.as_str())
            {
                members.push(SourceUpdateDraftMember {
                    plugin_name: None,
                    skill_id: member.skill_id.0.clone(),
                    skill_path: skill_path.clone(),
                    directory_name: member.directory_name.clone(),
                    directory_identity_key: member.identity_key.clone(),
                    display_name: member.display_name.clone(),
                    description: member.description.clone(),
                    tree_summary: String::new(),
                    state: SourceUpdateMemberState::Removed,
                });
            }
        }
        members.sort_by(|left, right| left.skill_path.cmp(&right.skill_path));
        Ok(SourceUpdateDraft {
            remote_id: remote_id.into(),
            provider: preview.provider,
            source_url: preview.source_url,
            aliases: current.aliases,
            policy: preview.policy,
            members,
        })
    }

    /// Confirmation: one whole-source Update through the same immutable
    /// Source Transition machinery (journal/CAS/recovery/Undo).
    pub fn confirm(
        &self,
        remote_id: &str,
        tracking_policy: Option<SourceTrackingOverride>,
        expected_selected_ref: String,
        expected_resolved_commit: String,
    ) -> Result<SourceTransitionResult, SourceUpdateError> {
        self.transition
            .confirm_update(ConfirmSourceUpdateRequest {
                remote_id: remote_id.into(),
                tracking_policy,
                expected_selected_ref,
                expected_resolved_commit,
            })
            .map_err(SourceUpdateError::Transition)
    }

    pub fn undo(&self, operation_id: &str) -> Result<SourceUndoResult, SourceUpdateError> {
        self.transition
            .undo(operation_id)
            .map_err(SourceUpdateError::Transition)
    }

    pub fn finalize(&self, operation_id: &str) -> Result<(), SourceUpdateError> {
        self.transition
            .finalize(operation_id)
            .map_err(SourceUpdateError::Transition)
    }

    /// Startup and pre-write verification of every current member snapshot
    /// against the immutable current Source Release. Mismatches persist
    /// `source_snapshot_mismatch` health and block Update/new Enable/
    /// ordinary source writes (read-only, Disable, Local Copy and explicit
    /// Restore stay available). Returns the number of mismatched members.
    pub fn verify_all_members(&self) -> Result<u32, SourceUpdateError> {
        let library_root = self.active_library_root()?;
        let mut mismatched = 0u32;
        for remote_id in self.transition.update_store_handle().source_ids()? {
            let Some(current) = self
                .transition
                .update_store_handle()
                .read_current(&remote_id)?
            else {
                continue;
            };
            let mut health = Vec::new();
            for member in &current.members {
                if member.presence != SourceMemberPresence::Current {
                    continue;
                }
                let namespace = library_root.join(&member.storage_relpath);
                let observed = self.observed_hash(&namespace)?;
                let matches = observed.as_deref() == member.tree_hash.as_deref();
                if !matches {
                    mismatched += 1;
                    if member.health != Health::SourceSnapshotMismatch {
                        health.push((member.skill_id.clone(), Health::SourceSnapshotMismatch));
                    }
                } else if member.health != Health::Healthy {
                    health.push((member.skill_id.clone(), Health::Healthy));
                }
            }
            if !health.is_empty() {
                self.transition
                    .update_store_handle()
                    .set_source_member_health(&remote_id, &health)?;
            }
        }
        Ok(mismatched)
    }

    /// New-Enable gate (spec §8.3): `source_snapshot_mismatch` blocks new
    /// Enable; a healthy member must also pass the byte-level re-verify.
    pub fn ensure_new_enable_allowed(&self, skill_id: &SkillId) -> Result<(), SourceUpdateError> {
        let library_root = self.active_library_root()?;
        let Some((remote_id, health)) = self
            .transition
            .update_store_handle()
            .member_health(skill_id)?
        else {
            // Non-Git skills have no snapshot gate.
            return Ok(());
        };
        if health == Health::SourceSnapshotMismatch {
            return Err(SourceUpdateError::SourceSnapshotMismatch);
        }
        let Some(current) = self
            .transition
            .update_store_handle()
            .read_current(&remote_id)?
        else {
            return Err(SourceUpdateError::SourceSnapshotMismatch);
        };
        let Some(member) = current
            .members
            .iter()
            .find(|member| member.skill_id.0 == skill_id.0)
        else {
            return Ok(());
        };
        if member.presence != SourceMemberPresence::Current {
            return Err(SourceUpdateError::Validation(
                "the Git Source Member is tombstoned; it cannot be enabled".into(),
            ));
        }
        let namespace = library_root.join(&member.storage_relpath);
        let observed = self.observed_hash(&namespace)?;
        if observed.as_deref() != member.tree_hash.as_deref() || member.health != Health::Healthy {
            return Err(SourceUpdateError::SourceSnapshotMismatch);
        }
        Ok(())
    }

    /// Startup recovery for Restore / Local Copy / Remove journals. It
    /// reads only the frozen journals; no remote is ever fetched again.
    fn observed_hash(&self, namespace: &Path) -> Result<Option<String>, SourceUpdateError> {
        match self.filesystem.staged_tree_snapshot(namespace) {
            Ok(snapshot) => Ok(Some(snapshot.content_hash)),
            Err(FileSystemError::Io { source, .. })
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn active_library_root(&self) -> Result<PathBuf, SourceUpdateError> {
        match &self.home_context {
            Some(context) => context
                .bound_home()
                .map(|home| home.path)
                .map_err(|error| SourceUpdateError::RecoveryRequired(error.to_string())),
            None => Ok(self.configured_library_root.clone()),
        }
    }
}
