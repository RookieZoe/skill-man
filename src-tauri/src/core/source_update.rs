//! Source-wide Git Repository Source Update draft classification.
//!
//! The v9 immutable Source Update (member add/remove/reappear, Source Member
//! Tombstone, Source Snapshot Mismatch, Create Local Source Copy and whole
//! source Remove) is ticket #93. This module keeps the read-only draft
//! contract working on the v9 facts — it classifies a freshly fetched
//! complete release against the managed source's current members — and
//! closes every write path until ticket #93 restores the update transition.

use std::sync::Arc;

use thiserror::Error;

use crate::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPolicyFacts, SourceGroupPreviewError,
    SourceGroupPreviewOutcome, SourceGroupPreviewService, SourceTrackingOverride,
};
use crate::seams::source_transition_store::{SourceTransitionStore, SourceTransitionStoreError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceUpdateMemberState {
    /// Already current in the managed source (same `skill_path`).
    Current,
    /// A member of the discovered release without a current counterpart.
    Added,
    /// A current member absent from the discovered release.
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdateDraftMember {
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

#[derive(Debug, Error)]
pub enum SourceUpdateError {
    #[error("Source Update confirmations are restored by ticket #93")]
    UpdateRestoredByTicket93,
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Preview(#[from] SourceGroupPreviewError),
    #[error(transparent)]
    Store(#[from] SourceTransitionStoreError),
}

/// Read-only classification of a fresh complete release against the managed
/// source; the update write machinery is ticket #93.
pub struct SourceUpdateService {
    preview: Arc<SourceGroupPreviewService>,
    store: Arc<dyn SourceTransitionStore>,
}

impl SourceUpdateService {
    pub fn new(
        preview: Arc<SourceGroupPreviewService>,
        store: Arc<dyn SourceTransitionStore>,
    ) -> Self {
        Self { preview, store }
    }

    pub fn preview(
        &self,
        remote_id: &str,
        tracking_policy: Option<SourceTrackingOverride>,
    ) -> Result<SourceUpdateDraft, SourceUpdateError> {
        // Re-read the managed source at preview time (never from the client).
        let existing = self.store.existing_source(remote_id)?.ok_or_else(|| {
            SourceUpdateError::Validation(
                "the selected Git Repository Source is no longer complete".into(),
            )
        })?;
        let canonical_url = existing.canonical_url.clone();
        let source_type = if canonical_url.starts_with("https://github.com/") {
            "github"
        } else if canonical_url.starts_with("https://gitlab.com/") {
            "gitlab"
        } else {
            "git"
        };
        let outcome = self
            .preview
            .fetch_latest_and_manage(FetchLatestAndManageRequest {
                source_type: source_type.into(),
                source_url: canonical_url,
                tracking_policy,
            })?;
        let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
            return Err(SourceUpdateError::Validation(
                "the current source draft is conflicted; resolve it and preview again".into(),
            ));
        };
        let current_by_path = existing
            .members
            .into_iter()
            .map(|member| (member.skill_path.clone(), member))
            .collect::<std::collections::BTreeMap<_, _>>();
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
                    .map(|member| member.skill_id.clone())
                    .unwrap_or_default();
                SourceUpdateDraftMember {
                    skill_id,
                    skill_path: member.skill_path.clone(),
                    directory_name: member.directory_name.clone(),
                    directory_identity_key: member.directory_identity_key.clone(),
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
            if !discovered_paths.contains(skill_path.as_str()) {
                members.push(SourceUpdateDraftMember {
                    skill_id: member.skill_id.clone(),
                    skill_path: skill_path.clone(),
                    directory_name: member.directory_name.clone(),
                    directory_identity_key: String::new(),
                    display_name: member.directory_name.clone(),
                    description: String::new(),
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
            aliases: preview.aliases,
            policy: preview.policy,
            members,
        })
    }

    /// Confirmation, Source-Transition recovery and the whole-source Update
    /// commit are ticket #93; every write stays closed here.
    pub fn confirm(&self, _remote_id: &str) -> Result<(), SourceUpdateError> {
        Err(SourceUpdateError::UpdateRestoredByTicket93)
    }
}
