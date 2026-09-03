//! Legacy Source Promotion draft classification and confirmation binding
//! (ADR-0014, spec §8.3).
//!
//! A Legacy Per-Skill Git State has only per-member evidence. It must never
//! be silently interpreted as a current Source Release. This module builds
//! the in-memory Source Group Draft that compares those legacy members with
//! a freshly discovered, complete release — then the same immutable Source
//! Transition (ticket #92) performs the single source-level commit. It has
//! no persistence seam: cancel and re-discovery leave Home, Catalog,
//! staging and journals alone.

use std::collections::BTreeMap;
use std::sync::Arc;

use thiserror::Error;

use crate::core::source_group_preview::{
    ExternalOwnershipClaim, FetchLatestAndManageRequest, SourceGroupPolicyFacts,
    SourceGroupPreviewError, SourceGroupPreviewOutcome, SourceGroupPreviewService,
    SourceTrackingOverride,
};
pub use crate::core::source_transition::ConfirmSourcePromotionRequest;
use crate::core::source_transition::{
    SourceTransitionError, SourceTransitionResult, SourceTransitionService, SourceUndoResult,
};
use crate::seams::source_promotion_store::{
    LegacySourcePromotionRecord, SourcePromotionStore, SourcePromotionStoreError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePromotionMemberState {
    /// Matches a legacy member by exact `skill_path`; keeps its skill_id.
    Current,
    /// A member of the target release without a legacy counterpart.
    Added,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionDraftMember {
    pub skill_path: String,
    pub directory_name: String,
    pub directory_identity_key: String,
    pub display_name: String,
    pub description: String,
    pub tree_summary: String,
    pub state: SourcePromotionMemberState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionRemovedMember {
    pub skill_id: String,
    pub directory_name: String,
    pub skill_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePromotionDraft {
    pub remote_id: String,
    pub provider: String,
    pub source_url: String,
    pub aliases: Vec<String>,
    pub policy: SourceGroupPolicyFacts,
    /// The complete manifest: every target member plus every removed legacy
    /// member (audit only; no rename/mapping is guessed).
    pub members: Vec<SourcePromotionDraftMember>,
    pub removed_members: Vec<SourcePromotionRemovedMember>,
    /// Frozen legacy audit facts (ref/commit/anchor/baseline are never
    /// current release truth).
    pub legacy: LegacySourcePromotionRecord,
    pub external_ownership_claims: Vec<ExternalOwnershipClaim>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourcePromotionDraftOutcome {
    Draft(Box<SourcePromotionDraft>),
    RepositoryRefConflict(crate::core::source_group_preview::RepositoryRefConflict),
    RepositoryOwnershipSplit(crate::core::source_group_preview::RepositoryOwnershipSplit),
}

#[derive(Debug, Error)]
pub enum SourcePromotionError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Store(#[from] SourcePromotionStoreError),
    #[error(transparent)]
    Preview(#[from] SourceGroupPreviewError),
    #[error(transparent)]
    Transition(#[from] SourceTransitionError),
}

/// The promotion preview is read-only; its confirmation is a Source
/// Transition (same journal/CAS/recovery/Undo as a clean transition).
pub struct SourcePromotionService {
    preview: Arc<SourceGroupPreviewService>,
    promotion_store: Arc<dyn SourcePromotionStore>,
    transition: Arc<SourceTransitionService>,
}

impl SourcePromotionService {
    pub fn new(
        preview: Arc<SourceGroupPreviewService>,
        promotion_store: Arc<dyn SourcePromotionStore>,
        transition: Arc<SourceTransitionService>,
    ) -> Self {
        Self {
            preview,
            promotion_store,
            transition,
        }
    }

    pub fn preview(
        &self,
        remote_id: &str,
        tracking_policy: Option<SourceTrackingOverride>,
    ) -> Result<SourcePromotionDraftOutcome, SourcePromotionError> {
        let legacy = self
            .promotion_store
            .read_legacy_source_promotion(remote_id)?;
        if legacy.members.is_empty() {
            return Err(SourcePromotionError::Validation(
                "the Legacy parent has no complete per-Skill member set".into(),
            ));
        }
        let source_type = provider_for_url(&legacy.canonical_url);
        let outcome = self
            .preview
            .fetch_latest_and_manage(FetchLatestAndManageRequest {
                source_type: source_type.into(),
                source_url: legacy.canonical_url.clone(),
                tracking_policy,
            })?;
        match outcome {
            SourceGroupPreviewOutcome::Preview(preview) => {
                let legacy_by_path = legacy
                    .members
                    .iter()
                    .map(|member| (member.skill_path.as_str(), member.skill_id.0.clone()))
                    .collect::<BTreeMap<_, _>>();
                let target_paths = preview
                    .members
                    .iter()
                    .map(|member| member.skill_path.as_str())
                    .collect::<std::collections::BTreeSet<_>>();
                let members = preview
                    .members
                    .iter()
                    .map(|member| SourcePromotionDraftMember {
                        skill_path: member.skill_path.clone(),
                        directory_name: member.directory_name.clone(),
                        directory_identity_key: member.directory_identity_key.clone(),
                        display_name: member.display_name.clone(),
                        description: member.description.clone(),
                        tree_summary: member.tree_summary.clone(),
                        state: if legacy_by_path.contains_key(member.skill_path.as_str()) {
                            SourcePromotionMemberState::Current
                        } else {
                            SourcePromotionMemberState::Added
                        },
                    })
                    .collect();
                let removed_members = legacy
                    .members
                    .iter()
                    .filter(|member| !target_paths.contains(member.skill_path.as_str()))
                    .map(|member| SourcePromotionRemovedMember {
                        skill_id: member.skill_id.0.clone(),
                        directory_name: member.directory_name.clone(),
                        skill_path: member.skill_path.clone(),
                    })
                    .collect();
                Ok(SourcePromotionDraftOutcome::Draft(Box::new(
                    SourcePromotionDraft {
                        remote_id: legacy.remote_id.clone(),
                        provider: preview.provider.clone(),
                        source_url: preview.source_url.clone(),
                        aliases: legacy.aliases.clone(),
                        policy: preview.policy,
                        members,
                        removed_members,
                        legacy,
                        external_ownership_claims: preview.external_ownership_claims,
                    },
                )))
            }
            SourceGroupPreviewOutcome::RepositoryRefConflict(conflict) => {
                Ok(SourcePromotionDraftOutcome::RepositoryRefConflict(conflict))
            }
            SourceGroupPreviewOutcome::RepositoryOwnershipSplit(split) => {
                Ok(SourcePromotionDraftOutcome::RepositoryOwnershipSplit(split))
            }
        }
    }

    pub fn confirm(
        &self,
        request: ConfirmSourcePromotionRequest,
    ) -> Result<SourceTransitionResult, SourcePromotionError> {
        self.transition
            .confirm_promotion(request)
            .map_err(Into::into)
    }

    pub fn undo(&self, operation_id: &str) -> Result<SourceUndoResult, SourcePromotionError> {
        self.transition.undo(operation_id).map_err(Into::into)
    }

    pub fn finalize(&self, operation_id: &str) -> Result<(), SourcePromotionError> {
        self.transition.finalize(operation_id).map_err(Into::into)
    }
}

fn provider_for_url(canonical_url: &str) -> &'static str {
    if canonical_url.starts_with("https://github.com/") {
        "github"
    } else if canonical_url.starts_with("https://gitlab.com/") {
        "gitlab"
    } else {
        "git"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::source_group_preview::SourceGroupMemberAction;

    #[test]
    fn provider_mapping_derives_from_the_canonical_url() {
        assert_eq!(provider_for_url("https://github.com/a/b"), "github");
        assert_eq!(provider_for_url("https://gitlab.com/a/b"), "gitlab");
        assert_eq!(provider_for_url("https://example.com/a/b"), "git");
        assert_eq!(
            provider_for_url("file:///tmp/repo"),
            "git",
            "unnamed providers map to generic Git"
        );
    }

    #[test]
    fn draft_member_action_matches_preview_actions() {
        // The v9 preview always discovers a full release; a promotion marks
        // exact skill_path matches Current and everything else Added. This
        // test pins the closed state vocabulary used by DTO serde.
        assert_eq!(
            SourceGroupMemberAction::Added,
            SourceGroupMemberAction::Added
        );
        assert!(matches!(
            SourcePromotionMemberState::Current,
            SourcePromotionMemberState::Current
        ));
    }
}
