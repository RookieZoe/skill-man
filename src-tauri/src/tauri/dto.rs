use serde::{Deserialize, Serialize};

use crate::core::agent_configuration::{
    AgentConfiguration, AgentConfigurationApplyResult, AgentConfigurationDraft,
    AgentConfigurationOrigin, AgentConfigurationPlan, AgentConfigurationPlanKind,
    AgentManagementSnapshot, AgentPreset, AgentRootDraft, AgentRootRole,
};
use crate::core::app_update::{
    AppUpdateCheck, AppUpdateOffer, CancelledAppUpdate, DownloadedAppUpdate,
};
use crate::core::domain::{
    ActivationObservedState, AgentKind, CatalogFilter, Compatibility, Health, SkillDetail,
    SkillSummary, SourceKind,
};
use crate::core::git_source_capability::{
    GitSourceCapabilityKind, GitSourceCapabilityReport, GitSourceCapabilitySource,
};
use crate::core::import::{
    FileImportCandidate, FileImportDiscovery, FileImportPreview, FileImportResult,
    FileImportSelectionPreview, FileImportSelectionResult, GitImportCandidate, GitImportDiscovery,
    GitImportPreview, GitImportResult, GitImportSelectionPreview, GitImportSelectionResult,
    LibraryConflict, LinkImportCandidate, LinkImportPreview, LinkImportResult,
};
use crate::core::maintenance::{
    ActivationHealthReport, RelocatePreview, RelocateResult, RemovePreview, RemoveResult,
};
use crate::core::observation::{
    ActivationHealthCounts, ActivationHealthRow, ActivationHealthSnapshot, DetectionSnapshot,
    ObservationAndScanSnapshot, ObservationKind, ObservationStatus, PresetDetectionState,
    PresetObservation, RootDetectionState, RootObservation, StartupProbeRootCounts,
    StartupProbeRow, StartupProbeSnapshot, StartupProbeTargetCounts,
};
use crate::core::source_group_preview::{
    ExternalOwnershipClaim, FetchLatestAndManageRequest, RepositoryOwnershipSplit,
    RepositoryRefConflict, SourceGroupMember, SourceGroupPreview, SourceGroupPreviewOutcome,
};
use crate::core::source_promotion::{
    ConfirmSourcePromotionRequest, ModifiedMemberResolution, SourcePromotionDraft,
    SourcePromotionExistingMemberDraft, SourcePromotionMemberState, SourcePromotionResolution,
    SourcePromotionResult, SourcePromotionTargetMemberDraft, UpstreamMemberRemovedResolution,
};
use crate::core::source_transition::{
    ConfirmSourceTransitionRequest, SourceTransitionResult, SourceUndoResult,
};
use crate::core::startup::{StartupAgent, StartupInfo};
use crate::core::update::{
    UpdateCheckGroup, UpdateCheckItem, UpdateCheckReport, UpdateItemResult, UpdatePlan,
    UpdatePlanItem, UpdateResult,
};
use crate::seams::preferences_store::{AppPreferences, PreferenceUpdates};

// -- Git Repository Source capability scan (ADR-0014, spec §8.3) --

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitSourceCapabilityKindDto {
    GitRepositorySource,
    LegacyPerSkillGitState,
    RemoteSourceIdentityConflict,
}

impl From<GitSourceCapabilityKind> for GitSourceCapabilityKindDto {
    fn from(value: GitSourceCapabilityKind) -> Self {
        match value {
            GitSourceCapabilityKind::GitRepositorySource => Self::GitRepositorySource,
            GitSourceCapabilityKind::LegacyPerSkillGitState => Self::LegacyPerSkillGitState,
            GitSourceCapabilityKind::RemoteSourceIdentityConflict => {
                Self::RemoteSourceIdentityConflict
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSourceCapabilitySourceDto {
    pub remote_id: String,
    pub canonical_url: String,
    pub kind: GitSourceCapabilityKindDto,
}

impl From<GitSourceCapabilitySource> for GitSourceCapabilitySourceDto {
    fn from(value: GitSourceCapabilitySource) -> Self {
        Self {
            remote_id: value.remote_id,
            canonical_url: value.canonical_url,
            kind: value.kind.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSourceCapabilityReportDto {
    pub sources: Vec<GitSourceCapabilitySourceDto>,
}

impl From<GitSourceCapabilityReport> for GitSourceCapabilityReportDto {
    fn from(value: GitSourceCapabilityReport) -> Self {
        Self {
            sources: value.sources.into_iter().map(Into::into).collect(),
        }
    }
}

// -- Fetch Latest and Manage Source Group Preview (ADR-0014, spec §8.4) --

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchLatestAndManageRequestDto {
    pub source_type: String,
    pub source_url: String,
    pub tracking_ref: Option<String>,
}

impl From<FetchLatestAndManageRequestDto> for FetchLatestAndManageRequest {
    fn from(value: FetchLatestAndManageRequestDto) -> Self {
        Self {
            source_type: value.source_type,
            source_url: value.source_url,
            tracking_ref: value.tracking_ref,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalOwnershipClaimDto {
    pub lock_path: String,
    pub entry_name: String,
    pub requested_ref: String,
}

impl From<ExternalOwnershipClaim> for ExternalOwnershipClaimDto {
    fn from(value: ExternalOwnershipClaim) -> Self {
        Self {
            lock_path: value.lock_path.to_string_lossy().into_owned(),
            entry_name: value.entry_name,
            requested_ref: value.requested_ref,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceGroupMemberDto {
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub skill_path: String,
    pub tree_summary: String,
}

impl From<SourceGroupMember> for SourceGroupMemberDto {
    fn from(value: SourceGroupMember) -> Self {
        Self {
            directory_name: value.directory_name,
            display_name: value.display_name,
            description: value.description,
            skill_path: value.skill_path,
            tree_summary: value.tree_summary,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceGroupPreviewDto {
    pub provider: String,
    pub source_url: String,
    pub tracking_ref: String,
    pub resolved_commit: String,
    pub members: Vec<SourceGroupMemberDto>,
    pub external_ownership_claims: Vec<ExternalOwnershipClaimDto>,
}

impl From<SourceGroupPreview> for SourceGroupPreviewDto {
    fn from(value: SourceGroupPreview) -> Self {
        Self {
            provider: value.provider,
            source_url: value.source_url,
            tracking_ref: value.tracking_ref,
            resolved_commit: value.resolved_commit,
            members: value.members.into_iter().map(Into::into).collect(),
            external_ownership_claims: value
                .external_ownership_claims
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryRefConflictDto {
    pub provider: String,
    pub source_url: String,
    pub available_refs: Vec<String>,
    pub external_ownership_claims: Vec<ExternalOwnershipClaimDto>,
}

impl From<RepositoryRefConflict> for RepositoryRefConflictDto {
    fn from(value: RepositoryRefConflict) -> Self {
        Self {
            provider: value.provider,
            source_url: value.source_url,
            available_refs: value.available_refs,
            external_ownership_claims: value
                .external_ownership_claims
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryOwnershipSplitDto {
    pub provider: String,
    pub source_url: String,
    pub tracking_ref: String,
    pub lock_paths: Vec<String>,
    pub external_ownership_claims: Vec<ExternalOwnershipClaimDto>,
}

impl From<RepositoryOwnershipSplit> for RepositoryOwnershipSplitDto {
    fn from(value: RepositoryOwnershipSplit) -> Self {
        Self {
            provider: value.provider,
            source_url: value.source_url,
            tracking_ref: value.tracking_ref,
            lock_paths: value
                .lock_paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            external_ownership_claims: value
                .external_ownership_claims
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceGroupPreviewOutcomeDto {
    Preview { preview: SourceGroupPreviewDto },
    RepositoryRefConflict { conflict: RepositoryRefConflictDto },
    RepositoryOwnershipSplit { split: RepositoryOwnershipSplitDto },
}

impl From<SourceGroupPreviewOutcome> for SourceGroupPreviewOutcomeDto {
    fn from(value: SourceGroupPreviewOutcome) -> Self {
        match value {
            SourceGroupPreviewOutcome::Preview(preview) => Self::Preview {
                preview: preview.into(),
            },
            SourceGroupPreviewOutcome::RepositoryRefConflict(conflict) => {
                Self::RepositoryRefConflict {
                    conflict: conflict.into(),
                }
            }
            SourceGroupPreviewOutcome::RepositoryOwnershipSplit(split) => {
                Self::RepositoryOwnershipSplit {
                    split: split.into(),
                }
            }
        }
    }
}

// -- Legacy Source Promotion Draft (ADR-0014, spec §8.3) --

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSourcePromotionRequestDto {
    pub remote_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePromotionMemberStateDto {
    UpdateToTarget,
    ModifiedMemberResolutionRequired,
    UpstreamMemberRemoved,
}

impl From<SourcePromotionMemberState> for SourcePromotionMemberStateDto {
    fn from(value: SourcePromotionMemberState) -> Self {
        match value {
            SourcePromotionMemberState::UpdateToTarget => Self::UpdateToTarget,
            SourcePromotionMemberState::ModifiedMemberResolutionRequired => {
                Self::ModifiedMemberResolutionRequired
            }
            SourcePromotionMemberState::UpstreamMemberRemoved => Self::UpstreamMemberRemoved,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePromotionExistingMemberDraftDto {
    pub skill_id: String,
    pub directory_name: String,
    pub skill_path: String,
    pub modified: bool,
    pub state: SourcePromotionMemberStateDto,
}

impl From<SourcePromotionExistingMemberDraft> for SourcePromotionExistingMemberDraftDto {
    fn from(value: SourcePromotionExistingMemberDraft) -> Self {
        Self {
            skill_id: value.member.skill_id.0,
            directory_name: value.member.directory_name,
            skill_path: value.member.skill_path,
            modified: value.member.current_tree_hash != value.member.current_baseline_hash,
            state: value.state.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePromotionTargetMemberDraftDto {
    pub member: SourceGroupMemberDto,
    pub legacy_skill_id: Option<String>,
}

impl From<SourcePromotionTargetMemberDraft> for SourcePromotionTargetMemberDraftDto {
    fn from(value: SourcePromotionTargetMemberDraft) -> Self {
        Self {
            member: value.member.into(),
            legacy_skill_id: value.legacy_skill_id.map(|id| id.0),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePromotionDraftDto {
    pub remote_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub tracking_ref: String,
    pub resolved_commit: String,
    pub existing_members: Vec<SourcePromotionExistingMemberDraftDto>,
    pub target_members: Vec<SourcePromotionTargetMemberDraftDto>,
}

impl From<SourcePromotionDraft> for SourcePromotionDraftDto {
    fn from(value: SourcePromotionDraft) -> Self {
        Self {
            remote_id: value.remote_id,
            provider: value.provider,
            canonical_url: value.canonical_url,
            tracking_ref: value.tracking_ref,
            resolved_commit: value.resolved_commit,
            existing_members: value.existing_members.into_iter().map(Into::into).collect(),
            target_members: value.target_members.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifiedMemberResolutionDto {
    KeepModified,
    ReplaceWithTarget,
}

impl From<ModifiedMemberResolutionDto> for ModifiedMemberResolution {
    fn from(value: ModifiedMemberResolutionDto) -> Self {
        match value {
            ModifiedMemberResolutionDto::KeepModified => Self::KeepModified,
            ModifiedMemberResolutionDto::ReplaceWithTarget => Self::ReplaceWithTarget,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpstreamMemberRemovedResolutionDto {
    Remove,
    LocalLink { target_directory: String },
    ExplicitMemberMapping { target_skill_path: String },
}

impl From<UpstreamMemberRemovedResolutionDto> for UpstreamMemberRemovedResolution {
    fn from(value: UpstreamMemberRemovedResolutionDto) -> Self {
        match value {
            UpstreamMemberRemovedResolutionDto::Remove => Self::Remove,
            UpstreamMemberRemovedResolutionDto::LocalLink { target_directory } => {
                Self::LocalLink { target_directory }
            }
            UpstreamMemberRemovedResolutionDto::ExplicitMemberMapping { target_skill_path } => {
                Self::ExplicitMemberMapping { target_skill_path }
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePromotionResolutionDto {
    pub skill_id: String,
    pub modified: Option<ModifiedMemberResolutionDto>,
    pub removed: Option<UpstreamMemberRemovedResolutionDto>,
}

impl From<SourcePromotionResolutionDto> for SourcePromotionResolution {
    fn from(value: SourcePromotionResolutionDto) -> Self {
        Self {
            skill_id: crate::core::domain::SkillId(value.skill_id),
            modified: value.modified.map(Into::into),
            removed: value.removed.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmSourcePromotionRequestDto {
    pub remote_id: String,
    pub expected_resolved_commit: String,
    pub resolutions: Vec<SourcePromotionResolutionDto>,
}

impl From<ConfirmSourcePromotionRequestDto> for ConfirmSourcePromotionRequest {
    fn from(value: ConfirmSourcePromotionRequestDto) -> Self {
        Self {
            remote_id: value.remote_id,
            expected_resolved_commit: value.expected_resolved_commit,
            resolutions: value.resolutions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePromotionResultDto {
    pub operation_id: String,
    pub remote_id: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub member_count: u32,
    pub snapshot_version: u64,
    pub undo_available: bool,
}

impl From<SourcePromotionResult> for SourcePromotionResultDto {
    fn from(value: SourcePromotionResult) -> Self {
        Self {
            operation_id: value.operation_id,
            remote_id: value.remote_id,
            release_id: value.release_id,
            resolved_commit: value.resolved_commit,
            member_count: value.member_count,
            snapshot_version: value.snapshot_version,
            undo_available: value.undo_available,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePromotionUndoResultDto {
    pub operation_id: String,
    pub member_count: u32,
    pub snapshot_version: u64,
}

impl From<crate::core::source_promotion::SourcePromotionUndoResult>
    for SourcePromotionUndoResultDto
{
    fn from(value: crate::core::source_promotion::SourcePromotionUndoResult) -> Self {
        Self {
            operation_id: value.operation_id,
            member_count: value.member_count,
            snapshot_version: value.snapshot_version,
        }
    }
}

// -- Git Repository Source Transition (ADR-0014, spec §8.4) --

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmSourceTransitionRequestDto {
    pub source_type: String,
    pub source_url: String,
    pub tracking_ref: String,
    pub expected_resolved_commit: String,
}

impl From<ConfirmSourceTransitionRequestDto> for ConfirmSourceTransitionRequest {
    fn from(value: ConfirmSourceTransitionRequestDto) -> Self {
        Self {
            source_type: value.source_type,
            source_url: value.source_url,
            tracking_ref: value.tracking_ref,
            expected_resolved_commit: value.expected_resolved_commit,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTransitionOperationRequestDto {
    pub operation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTransitionResultDto {
    pub operation_id: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub member_count: u32,
    pub snapshot_version: u64,
    pub undo_available: bool,
}

impl From<SourceTransitionResult> for SourceTransitionResultDto {
    fn from(value: SourceTransitionResult) -> Self {
        Self {
            operation_id: value.operation_id,
            release_id: value.release_id,
            resolved_commit: value.resolved_commit,
            member_count: value.member_count,
            snapshot_version: value.snapshot_version,
            undo_available: value.undo_available,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceUndoResultDto {
    pub operation_id: String,
    pub member_count: u32,
    pub snapshot_version: u64,
}

impl From<SourceUndoResult> for SourceUndoResultDto {
    fn from(value: SourceUndoResult) -> Self {
        Self {
            operation_id: value.operation_id,
            member_count: value.member_count,
            snapshot_version: value.snapshot_version,
        }
    }
}

// -- Skill Man application Update (ADR-0006) --

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckAppUpdateRequestDto {
    pub force: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AppUpdateCheckDto {
    #[serde(rename = "skipped")]
    Skipped,
    UpToDate,
    Available {
        #[serde(rename = "updateId")]
        update_id: String,
        #[serde(rename = "currentVersion")]
        current_version: String,
        version: String,
        #[serde(rename = "releaseNotes")]
        release_notes: String,
        #[serde(rename = "downloadSizeBytes")]
        download_size_bytes: u64,
    },
}

impl From<AppUpdateCheck> for AppUpdateCheckDto {
    fn from(value: AppUpdateCheck) -> Self {
        match value {
            AppUpdateCheck::SkippedCooldown => Self::Skipped,
            AppUpdateCheck::UpToDate => Self::UpToDate,
            AppUpdateCheck::Available(AppUpdateOffer {
                update_id,
                current_version,
                version,
                release_notes,
                download_size_bytes,
            }) => Self::Available {
                update_id,
                current_version,
                version,
                release_notes,
                download_size_bytes,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadAppUpdateRequestDto {
    pub update_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadedAppUpdateDto {
    pub update_id: String,
    pub version: String,
}

impl From<DownloadedAppUpdate> for DownloadedAppUpdateDto {
    fn from(value: DownloadedAppUpdate) -> Self {
        Self {
            update_id: value.update_id,
            version: value.version,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelAppUpdateRequestDto {
    pub update_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledAppUpdateDto {
    pub update_id: String,
}

impl From<CancelledAppUpdate> for CancelledAppUpdateDto {
    fn from(value: CancelledAppUpdate) -> Self {
        Self {
            update_id: value.update_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallAppUpdateRequestDto {
    pub update_id: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFilterDto {
    #[default]
    All,
    Broken,
    Modified,
    Link,
    Install,
}

impl From<CatalogFilterDto> for CatalogFilter {
    fn from(value: CatalogFilterDto) -> Self {
        match value {
            CatalogFilterDto::All => Self::All,
            CatalogFilterDto::Broken => Self::Broken,
            CatalogFilterDto::Modified => Self::Modified,
            CatalogFilterDto::Link => Self::Link,
            CatalogFilterDto::Install => Self::Install,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSkillsRequestDto {
    pub filter: CatalogFilterDto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKindDto {
    Link,
    RemoteInstall,
    FileInstall,
}

impl From<SourceKind> for SourceKindDto {
    fn from(value: SourceKind) -> Self {
        match value {
            SourceKind::Link => Self::Link,
            SourceKind::RemoteInstall => Self::RemoteInstall,
            SourceKind::FileInstall => Self::FileInstall,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthDto {
    Healthy,
    Broken,
    Modified,
}

impl From<Health> for HealthDto {
    fn from(value: Health) -> Self {
        match value {
            Health::Healthy => Self::Healthy,
            Health::Broken => Self::Broken,
            Health::Modified => Self::Modified,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummaryDto {
    pub id: String,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub source_kind: SourceKindDto,
    pub health: HealthDto,
    pub enabled_agent_count: u32,
}

impl From<SkillSummary> for SkillSummaryDto {
    fn from(value: SkillSummary) -> Self {
        Self {
            id: value.id.0,
            directory_name: value.directory_name,
            display_name: value.display_name,
            description: value.description,
            source_kind: value.source_kind.into(),
            health: value.health.into(),
            enabled_agent_count: value.enabled_agent_count,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogListDto {
    pub snapshot_version: u64,
    pub items: Vec<SkillSummaryDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetailDto {
    pub id: String,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub source_kind: SourceKindDto,
    pub health: HealthDto,
    pub enabled_agent_count: u32,
    pub final_entity_path: String,
    /// Raw Source Content: the original file Install path (never App Copy).
    pub file_source_original_path: Option<String>,
    pub frontmatter_name: Option<String>,
    pub last_activity_at: String,
    pub skill_markdown: String,
}

impl From<SkillDetail> for SkillDetailDto {
    fn from(value: SkillDetail) -> Self {
        let summary = value.summary;
        Self {
            id: summary.id.0,
            directory_name: summary.directory_name,
            display_name: summary.display_name,
            description: summary.description,
            source_kind: summary.source_kind.into(),
            health: summary.health.into(),
            enabled_agent_count: summary.enabled_agent_count,
            final_entity_path: value.final_entity_path,
            file_source_original_path: value.file_source_original_path,
            frontmatter_name: value.frontmatter_name,
            last_activity_at: value.last_activity_at,
            skill_markdown: value.skill_markdown,
        }
    }
}

// -- Agent Configuration Core (spec §3.4, §4.7; ADR-0016) --

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentConfigurationOriginDto {
    Preset,
    Custom,
}

impl From<AgentConfigurationOrigin> for AgentConfigurationOriginDto {
    fn from(value: AgentConfigurationOrigin) -> Self {
        match value {
            AgentConfigurationOrigin::Preset => Self::Preset,
            AgentConfigurationOrigin::Custom => Self::Custom,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRootRoleDto {
    ScanOnly,
    ActivationTarget,
}

impl From<AgentRootRole> for AgentRootRoleDto {
    fn from(value: AgentRootRole) -> Self {
        match value {
            AgentRootRole::ScanOnly => Self::ScanOnly,
            AgentRootRole::ActivationTarget => Self::ActivationTarget,
        }
    }
}

impl From<AgentRootRoleDto> for AgentRootRole {
    fn from(value: AgentRootRoleDto) -> Self {
        match value {
            AgentRootRoleDto::ScanOnly => Self::ScanOnly,
            AgentRootRoleDto::ActivationTarget => Self::ActivationTarget,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRootDraftDto {
    pub configured_path: String,
    pub role: AgentRootRoleDto,
}

impl From<AgentRootDraftDto> for AgentRootDraft {
    fn from(value: AgentRootDraftDto) -> Self {
        Self {
            configured_path: value.configured_path.into(),
            role: value.role.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentConfigurationRequestDto {
    pub preset_key: Option<String>,
    pub name: String,
    pub roots: Vec<AgentRootDraftDto>,
    pub project_skills_dir: Option<String>,
}

impl From<CreateAgentConfigurationRequestDto> for AgentConfigurationDraft {
    fn from(value: CreateAgentConfigurationRequestDto) -> Self {
        Self {
            preset_key: value.preset_key,
            name: value.name,
            roots: value.roots.into_iter().map(Into::into).collect(),
            project_skills_dir: value.project_skills_dir.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditAgentConfigurationRequestDto {
    pub agent_id: String,
    pub preset_key: Option<String>,
    pub name: String,
    pub roots: Vec<AgentRootDraftDto>,
    pub project_skills_dir: Option<String>,
}

impl EditAgentConfigurationRequestDto {
    pub fn into_parts(self) -> (String, AgentConfigurationDraft) {
        (
            self.agent_id,
            AgentConfigurationDraft {
                preset_key: self.preset_key,
                name: self.name,
                roots: self.roots.into_iter().map(Into::into).collect(),
                project_skills_dir: self.project_skills_dir.map(Into::into),
            },
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAgentConfigurationRequestDto {
    pub agent_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyAgentConfigurationPlanRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigurationRootDto {
    pub root_id: String,
    pub configured_path: String,
    pub path_identity_key: String,
    pub role: AgentRootRoleDto,
    pub consumer_agent_ids: Vec<String>,
    pub activation_skill_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigurationDto {
    pub agent_id: String,
    pub origin: AgentConfigurationOriginDto,
    pub preset_key: Option<String>,
    pub name: String,
    pub compatibility: CompatibilityDto,
    pub project_skills_dir: Option<String>,
    pub roots: Vec<AgentConfigurationRootDto>,
}

impl From<AgentConfiguration> for AgentConfigurationDto {
    fn from(value: AgentConfiguration) -> Self {
        Self {
            agent_id: value.agent_id,
            origin: value.origin.into(),
            preset_key: value.preset_key,
            name: value.name,
            compatibility: value.compatibility.into(),
            project_skills_dir: value
                .project_skills_dir
                .map(|path| path.to_string_lossy().into_owned()),
            roots: value
                .roots
                .into_iter()
                .map(|root| AgentConfigurationRootDto {
                    root_id: root.root_id,
                    configured_path: root.configured_path.to_string_lossy().into_owned(),
                    path_identity_key: root.path_identity_key,
                    role: root.role.into(),
                    consumer_agent_ids: root.consumer_agent_ids,
                    activation_skill_ids: root.activation_skill_ids,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetDto {
    pub preset_key: String,
    pub name: String,
    pub compatibility: CompatibilityDto,
    pub roots: Vec<String>,
    pub activation_target: String,
    pub project_skills_dir: String,
}

impl From<AgentPreset> for AgentPresetDto {
    fn from(value: AgentPreset) -> Self {
        Self {
            preset_key: value.preset_key,
            name: value.name,
            compatibility: value.compatibility.into(),
            roots: value
                .roots
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            activation_target: value.activation_target.to_string_lossy().into_owned(),
            project_skills_dir: value.project_skills_dir.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentManagementSnapshotDto {
    pub generation: u64,
    pub configurations: Vec<AgentConfigurationDto>,
    pub presets: Vec<AgentPresetDto>,
}

impl From<AgentManagementSnapshot> for AgentManagementSnapshotDto {
    fn from(value: AgentManagementSnapshot) -> Self {
        Self {
            generation: value.generation,
            configurations: value.configurations.into_iter().map(Into::into).collect(),
            presets: value.presets.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentConfigurationPlanKindDto {
    Create,
    Edit,
    Delete,
}

impl From<AgentConfigurationPlanKind> for AgentConfigurationPlanKindDto {
    fn from(value: AgentConfigurationPlanKind) -> Self {
        match value {
            AgentConfigurationPlanKind::Create => Self::Create,
            AgentConfigurationPlanKind::Edit => Self::Edit,
            AgentConfigurationPlanKind::Delete => Self::Delete,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigurationPlanDto {
    pub plan_token: String,
    pub kind: AgentConfigurationPlanKindDto,
    pub configuration: Option<AgentConfigurationDto>,
    pub target_will_be_created: bool,
    pub blocking_activation_skill_ids: Vec<String>,
    pub retained_activation_count: usize,
}

impl From<AgentConfigurationPlan> for AgentConfigurationPlanDto {
    fn from(value: AgentConfigurationPlan) -> Self {
        Self {
            plan_token: value.plan_token,
            kind: value.kind.into(),
            configuration: value.configuration.map(Into::into),
            target_will_be_created: value.target_will_be_created,
            blocking_activation_skill_ids: value.blocking_activation_skill_ids,
            retained_activation_count: value.retained_activation_count,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigurationApplyResultDto {
    pub agent_id: String,
    pub generation: u64,
    pub deleted: bool,
    /// Target Root ids affected by the apply: the caller schedules
    /// Target-scoped Activation health for exactly these (spec §4.10).
    pub affected_target_root_ids: Vec<String>,
}

impl From<AgentConfigurationApplyResult> for AgentConfigurationApplyResultDto {
    fn from(value: AgentConfigurationApplyResult) -> Self {
        Self {
            agent_id: value.agent_id,
            generation: value.generation,
            deleted: value.deleted,
            affected_target_root_ids: value.affected_target_root_ids,
        }
    }
}

// -- Observation and Scan Module (spec §4.10; ADR-0020) --

/// Closed per-root Detection observation state; probe failures are
/// `Unavailable` with a diagnostic, never downgraded to `Absent`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootDetectionStateDto {
    Present,
    Unavailable,
    Absent,
}

impl From<RootDetectionState> for RootDetectionStateDto {
    fn from(value: RootDetectionState) -> Self {
        match value {
            RootDetectionState::Present => Self::Present,
            RootDetectionState::Unavailable => Self::Unavailable,
            RootDetectionState::Absent => Self::Absent,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootObservationDto {
    pub configured_path: String,
    pub state: RootDetectionStateDto,
    pub canonical_path: Option<String>,
    pub diagnostic: Option<String>,
}

impl From<RootObservation> for RootObservationDto {
    fn from(value: RootObservation) -> Self {
        Self {
            configured_path: value.configured_path.to_string_lossy().into_owned(),
            state: value.state.into(),
            canonical_path: value
                .canonical_path
                .map(|path| path.to_string_lossy().into_owned()),
            diagnostic: value.diagnostic,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetDetectionStateDto {
    Present,
    Unavailable,
    Absent,
    Unknown,
}

impl From<PresetDetectionState> for PresetDetectionStateDto {
    fn from(value: PresetDetectionState) -> Self {
        match value {
            PresetDetectionState::Present => Self::Present,
            PresetDetectionState::Unavailable => Self::Unavailable,
            PresetDetectionState::Absent => Self::Absent,
            PresetDetectionState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetObservationDto {
    pub preset_key: String,
    pub name: String,
    pub state: PresetDetectionStateDto,
    pub roots: Vec<RootObservationDto>,
}

impl From<PresetObservation> for PresetObservationDto {
    fn from(value: PresetObservation) -> Self {
        Self {
            preset_key: value.preset_key,
            name: value.name,
            state: value.state.into(),
            roots: value.roots.into_iter().map(Into::into).collect(),
        }
    }
}

/// In-memory Detection result: bounded to the nine fixed Presets, generation
/// increments on every published run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionSnapshotDto {
    pub generation: u64,
    pub preset_observations: Vec<PresetObservationDto>,
}

impl From<DetectionSnapshot> for DetectionSnapshotDto {
    fn from(value: DetectionSnapshot) -> Self {
        Self {
            generation: value.generation,
            preset_observations: value
                .preset_observations
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

/// Observation lifecycle; `stale` is the cross-startup / kept-old view.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatusDto {
    Unknown,
    Checking,
    Observed,
    Stale,
}

impl From<ObservationStatus> for ObservationStatusDto {
    fn from(value: ObservationStatus) -> Self {
        match value {
            ObservationStatus::Unknown => Self::Unknown,
            ObservationStatus::Checking => Self::Checking,
            ObservationStatus::Observed => Self::Observed,
            ObservationStatus::Stale => Self::Stale,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupProbeRootCountsDto {
    pub total: u64,
    pub present: u64,
    pub unavailable: u64,
    pub absent: u64,
}

impl From<StartupProbeRootCounts> for StartupProbeRootCountsDto {
    fn from(value: StartupProbeRootCounts) -> Self {
        Self {
            total: value.total,
            present: value.present,
            unavailable: value.unavailable,
            absent: value.absent,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupProbeTargetCountsDto {
    pub total: u64,
    pub present: u64,
    pub unavailable: u64,
    pub absent: u64,
}

impl From<StartupProbeTargetCounts> for StartupProbeTargetCountsDto {
    fn from(value: StartupProbeTargetCounts) -> Self {
        Self {
            total: value.total,
            present: value.present,
            unavailable: value.unavailable,
            absent: value.absent,
        }
    }
}

/// Bounded Startup Probe summary (spec §4.10).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupProbeSnapshotDto {
    pub generation: u64,
    pub status: ObservationStatusDto,
    pub root_counts: StartupProbeRootCountsDto,
    pub target_counts: StartupProbeTargetCountsDto,
    pub slow: bool,
    pub diagnostic: Option<String>,
}

impl From<StartupProbeSnapshot> for StartupProbeSnapshotDto {
    fn from(value: StartupProbeSnapshot) -> Self {
        Self {
            generation: value.generation,
            status: value.status.into(),
            root_counts: value.root_counts.into(),
            target_counts: value.target_counts.into(),
            slow: value.slow,
            diagnostic: value.diagnostic,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationHealthCountsDto {
    pub target_groups: u64,
    pub observed_groups: u64,
    pub failed_groups: u64,
    pub unresponsive_groups: u64,
    pub entries_total: u64,
    pub entries_present: u64,
    pub entries_unhealthy: u64,
    pub entries_unknown: u64,
}

impl From<ActivationHealthCounts> for ActivationHealthCountsDto {
    fn from(value: ActivationHealthCounts) -> Self {
        Self {
            target_groups: value.target_groups,
            observed_groups: value.observed_groups,
            failed_groups: value.failed_groups,
            unresponsive_groups: value.unresponsive_groups,
            entries_total: value.entries_total,
            entries_present: value.entries_present,
            entries_unhealthy: value.entries_unhealthy,
            entries_unknown: value.entries_unknown,
        }
    }
}

/// Bounded Activation Health summary (spec §4.10).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationHealthSnapshotDto {
    pub generation: u64,
    pub status: ObservationStatusDto,
    pub target_group_counts: ActivationHealthCountsDto,
    pub slow: bool,
    pub diagnostic: Option<String>,
}

impl From<ActivationHealthSnapshot> for ActivationHealthSnapshotDto {
    fn from(value: ActivationHealthSnapshot) -> Self {
        Self {
            generation: value.generation,
            status: value.status.into(),
            target_group_counts: value.target_group_counts.into(),
            slow: value.slow,
            diagnostic: value.diagnostic,
        }
    }
}

/// The observation page contract (spec §4.10 `observation_page`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKindDto {
    StartupProbe,
    ActivationHealth,
}

impl From<ObservationKind> for ObservationKindDto {
    fn from(value: ObservationKind) -> Self {
        match value {
            ObservationKind::StartupProbe => Self::StartupProbe,
            ObservationKind::ActivationHealth => Self::ActivationHealth,
        }
    }
}

impl From<ObservationKindDto> for ObservationKind {
    fn from(value: ObservationKindDto) -> Self {
        match value {
            ObservationKindDto::StartupProbe => ObservationKind::StartupProbe,
            ObservationKindDto::ActivationHealth => ObservationKind::ActivationHealth,
        }
    }
}

/// Stable cursor into one observation generation (spec §4.10).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationCursorDto {
    pub offset: u64,
}

/// One Startup Probe row: configured Root/Target existence, readability,
/// path identity (read-only observation).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupProbeRowDto {
    pub configured_path: String,
    pub path_identity_key: String,
    pub canonical_path: Option<String>,
    /// `present | unavailable | absent`.
    pub state: String,
    pub diagnostic: Option<String>,
    pub is_target: bool,
}

impl From<StartupProbeRow> for StartupProbeRowDto {
    fn from(value: StartupProbeRow) -> Self {
        Self {
            configured_path: value.configured_path.to_string_lossy().into_owned(),
            path_identity_key: value.path_identity_key,
            canonical_path: value
                .canonical_path
                .map(|path| path.to_string_lossy().into_owned()),
            state: root_detection_state_name(value.state).to_owned(),
            diagnostic: value.diagnostic,
            is_target: value.is_target,
        }
    }
}

/// One Activation health row (an enabled `(Skill, Target)` activation).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationHealthRowDto {
    pub skill_id: String,
    pub target_root_id: String,
    pub entry_path: String,
    /// This generation's observation: `present | missing | target_mismatch
    /// | dangling | occupied`, `None` = Unknown.
    pub observed_state: Option<String>,
    /// The previously persisted observation (kept old on failure).
    pub previous_state: Option<String>,
    pub stale: bool,
    pub diagnostic: Option<String>,
    pub checked_at_ms: Option<u64>,
}

impl From<ActivationHealthRow> for ActivationHealthRowDto {
    fn from(value: ActivationHealthRow) -> Self {
        Self {
            skill_id: value.skill_id,
            target_root_id: value.target_root_id,
            entry_path: value.entry_path.to_string_lossy().into_owned(),
            observed_state: value
                .observed_state
                .map(|state| activation_observed_state_name(state).to_owned()),
            previous_state: value
                .previous_state
                .map(|state| activation_observed_state_name(state).to_owned()),
            stale: value.stale,
            diagnostic: value.diagnostic,
            checked_at_ms: value.checked_at_ms,
        }
    }
}

/// One paged observation row. Tagged on the wire: `{"kind":
/// "startup_probe"|"activation_health", "row": {...}}`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObservationRowDto {
    StartupProbe { row: StartupProbeRowDto },
    ActivationHealth { row: ActivationHealthRowDto },
}

/// A bounded page of one observation generation (spec §4.10).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationPageReadDto {
    pub generation: u64,
    pub rows: Vec<ObservationRowDto>,
    pub next_offset: Option<u64>,
}

/// Bounded per-Root view while a Run is active (spec §4.10).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRootViewDto {
    pub index: u32,
    pub configured_path: String,
    pub canonical_path: String,
    /// `pending | walking | completed | failed | unresponsive`.
    pub state: String,
    pub counts: ScanCountsDto,
    pub elapsed_ms: u64,
    pub slow: bool,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanCountsDto {
    pub roots: u64,
    pub entries: u64,
    pub entities: u64,
    pub files: u64,
    pub bytes: u64,
    pub git_probes: u64,
    pub failed_roots: u64,
    /// Funnel: configured Agent Configurations contributing roots.
    pub configured_agents: u64,
    /// Funnel: configured Root declarations before the canonical union.
    pub declared_roots: u64,
    /// Funnel: distinct physical Roots after the canonical union.
    pub canonical_roots: u64,
}

/// Scan Run snapshot: only real phase/Root/count/elapsed facts — never a
/// percent or ETA (ADR-0020).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRunSnapshotDto {
    pub run_id: String,
    pub generation: u64,
    /// `onboarding | manual`.
    pub trigger: String,
    /// `queued | running | cancelling | cancelled | superseded |
    /// completed | failed`.
    pub state: String,
    pub phase: String,
    pub current_root: Option<u32>,
    pub counts: ScanCountsDto,
    pub roots: Vec<ScanRootViewDto>,
    pub elapsed_ms: u64,
    pub slow: bool,
    pub diagnostic: Option<String>,
}

/// Layered Root coverage of a terminal Report (spec §4.6 `coverageCounts`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanCoverageCountsDto {
    pub completed: u64,
    pub failed: u64,
    pub unresponsive: u64,
}

/// Bounded terminal Report summary (spec §4.6): full Root coverage /
/// entity / appearance / diagnostic detail is served by the unique
/// `get_scan_report_page` contract, never by this DTO.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReportSummaryDto {
    pub generation: u64,
    pub run_id: String,
    /// The unforgeable Report identity page cursors bind to.
    pub content_identity: String,
    pub trigger: String,
    /// `complete | incomplete`.
    pub state: String,
    pub coverage: ScanCoverageCountsDto,
    pub counts: ScanCountsDto,
    pub incomplete: bool,
    pub published_at_ms: u64,
    pub agent_configuration_generation: u64,
    pub configured_root_snapshot_fingerprint: String,
    pub started_at_ms: u64,
    pub slow: bool,
    /// Bounded source classification counts (spec §8.1).
    pub source_counts: ScanSourceCountsDto,
}

/// Bounded source classification counts of a terminal Report (spec §8.1):
/// the four summary cards + attention tallies; candidate detail is paged.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSourceCountsDto {
    pub git_groups: u64,
    pub git_groups_conflicted: u64,
    pub local_candidates: u64,
    pub conflict_sets: u64,
    pub conflict_members: u64,
    pub blocked: u64,
    pub deferred: u64,
    pub identity_conflicts: u64,
    pub already_managed: u64,
    pub excluded: u64,
    pub needs_attention: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportFreshnessDto {
    Current,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleReasonDto {
    CrossStartup,
    ConfigurationChanged,
    HomeOrGateChanged,
    FilesystemChanged,
    CacheUnreadable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentReportDto {
    pub summary: Option<ScanReportSummaryDto>,
    pub freshness: ReportFreshnessDto,
    pub stale_reasons: Vec<StaleReasonDto>,
}

/// The `observation://changed` payload and the query snapshot are isomorphic
/// (spec §4.10): the same bounded summary, same generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationAndScanSnapshotDto {
    pub home_id: Option<String>,
    pub write_gate_generation: u64,
    pub agent_configuration_generation: Option<u64>,
    pub detection: DetectionSnapshotDto,
    /// Startup Probe bounded summary (spec §4.10); absent while the Agent
    /// Configuration is not readable.
    pub startup_probe: Option<StartupProbeSnapshotDto>,
    /// Activation Health bounded summary (spec §4.10).
    pub activation_health: Option<ActivationHealthSnapshotDto>,
    pub scan_run: Option<ScanRunSnapshotDto>,
    pub current_report: CurrentReportDto,
}

/// Generation-bound object identity (ADR-0017): valid only inside one Scan
/// Report generation — never a Skill identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanObjectIdentityDto {
    pub device: u64,
    pub inode: u64,
}

/// One bounded chain hop of an appearance (spec §8.1).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanChainHopDto {
    pub path: String,
    /// `directory` | `symlink:<target>`.
    pub kind: String,
    pub device: u64,
    pub inode: u64,
    pub target: Option<String>,
}

/// The typed fault that stopped the bounded chain (never a guess after).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanChainFaultDto {
    /// `dangling | cycle | hop_limit | non_utf8 | read_failed |
    /// not_directory | identity_replaced`.
    pub kind: String,
    pub at: String,
    pub detail: Option<String>,
}

/// One row of a Report page; the section determines the variant (spec
/// §4.7: closed `kind` + typed fields, never free-form App Copy).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanReportRowDto {
    RootCoverage {
        index: u32,
        configured_path: String,
        canonical_path: String,
        /// `completed | failed | unresponsive`.
        state: String,
        /// Consumer Agents of this Root (id + display name).
        consumer_agents: Vec<ScanRootAgentDto>,
        counts: ScanCountsDto,
        elapsed_ms: u64,
        slow: bool,
        diagnostic: Option<String>,
    },
    Entity {
        entity_seq: u64,
        identity: ScanObjectIdentityDto,
        canonical_path: String,
        file_count: u64,
        byte_count: u64,
        tree_hash: Option<String>,
        hash_fault: Option<String>,
        appearances: u64,
        first_root_index: u32,
        first_entry_seq: u64,
    },
    Appearance {
        root_index: u32,
        seq: u64,
        name: String,
        entry_path: String,
        /// `directory | symlink`.
        entry_kind: String,
        chain: Vec<ScanChainHopDto>,
        chain_fault: Option<ScanChainFaultDto>,
        final_entity: Option<String>,
        identity: Option<ScanObjectIdentityDto>,
        entity_seq: Option<u64>,
        lock_hint: Box<Option<ScanLockHintDto>>,
        worktree_hint: Option<ScanWorktreeHintDto>,
    },
    GitSourceGroup {
        group_seq: u64,
        provider: String,
        canonical_repository: String,
        repository_root: Option<String>,
        remote_urls_seen: Vec<String>,
        member_entity_seqs: Vec<u64>,
        member_paths: Vec<String>,
        member_names: Vec<String>,
        lock_claims: Vec<ScanLockClaimDto>,
        refs: Vec<String>,
        lock_paths: Vec<String>,
        /// `candidate | repository_ref_conflict | ownership_split`.
        status: String,
        operations: Vec<ScanOperationEligibilityDto>,
        detail: Option<String>,
    },
    SourceVerdict {
        entity_seq: u64,
        /// `local | git | conflict_set | identity_conflict | blocked |
        /// deferred | already_managed | excluded`.
        verdict: String,
        canonical_path: String,
        directory_names: Vec<String>,
        appearances: u64,
        file_count: u64,
        byte_count: u64,
        tree_hash: Option<String>,
        lock_claims: Vec<ScanLockClaimDto>,
        worktree_hints: Vec<ScanWorktreeHintDto>,
        reason_kind: Option<String>,
        detail: Option<String>,
        git_refs: Vec<String>,
        git_lock_paths: Vec<String>,
        git_group_seq: Option<u64>,
        conflict_set_seq: Option<u64>,
        notes: Vec<String>,
        operations: Vec<ScanOperationEligibilityDto>,
    },
    ConflictSet {
        set_seq: u64,
        directory_identity_key: String,
        directory_name: String,
        member_entity_seqs: Vec<u64>,
        member_paths: Vec<String>,
        winner_entity_seq: Option<u64>,
    },
    Diagnostic {
        root_index: u32,
        /// `root_failed | root_unresponsive | root_record_missing |
        /// chain_<fault> | entity_hash_fault | entity_identity_fault`.
        diagnostic_kind: String,
        at: Option<String>,
        detail: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRootAgentDto {
    pub agent_id: String,
    pub agent_name: String,
}

/// Memoized lock fact of one appearance (never the lock body).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanLockHintDto {
    pub lock_path: String,
    pub entry_name: String,
    pub fingerprint: String,
    pub faulted: bool,
    pub fault: Option<String>,
    pub source_type: Option<String>,
    pub source_url: Option<String>,
    pub requested_ref: Option<String>,
    pub skill_path: Option<String>,
}

/// One enriched applicable lock claim (never the lock body).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanLockClaimDto {
    pub lock_path: String,
    pub entry_name: String,
    pub fingerprint: String,
    pub source_type: Option<String>,
    pub source_url: Option<String>,
    pub requested_ref: Option<String>,
    pub skill_path: Option<String>,
}

/// Typed operation eligibility of one candidate (spec §8.1).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOperationEligibilityDto {
    pub operation: String,
    pub allowed: bool,
    pub closed_reason: Option<String>,
}

/// Memoized bounded local Git worktree hint of one appearance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanWorktreeHintDto {
    pub repository_root: String,
    /// `dir | file | uninterpretable`.
    pub gitdir_kind: String,
    pub remote_urls: Vec<String>,
    pub head_ref: Option<String>,
}

/// A stable Report cursor (spec §4.10): pinned to one Report identity;
/// `offset` is a row index for `roots` and a byte offset into the immutable
/// index stream for `entities | appearances | diagnostics`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanReportSectionDto {
    Roots,
    Entities,
    Appearances,
    Diagnostics,
    GitSources,
    LocalCandidates,
    ConflictSets,
    NeedsAttention,
    Excluded,
}

impl From<ScanReportSectionDto> for crate::seams::scan_evidence_store::ScanReportSection {
    fn from(value: ScanReportSectionDto) -> Self {
        match value {
            ScanReportSectionDto::Roots => {
                crate::seams::scan_evidence_store::ScanReportSection::Roots
            }
            ScanReportSectionDto::Entities => {
                crate::seams::scan_evidence_store::ScanReportSection::Entities
            }
            ScanReportSectionDto::Appearances => {
                crate::seams::scan_evidence_store::ScanReportSection::Appearances
            }
            ScanReportSectionDto::Diagnostics => {
                crate::seams::scan_evidence_store::ScanReportSection::Diagnostics
            }
            ScanReportSectionDto::GitSources => {
                crate::seams::scan_evidence_store::ScanReportSection::GitSources
            }
            ScanReportSectionDto::LocalCandidates => {
                crate::seams::scan_evidence_store::ScanReportSection::LocalCandidates
            }
            ScanReportSectionDto::ConflictSets => {
                crate::seams::scan_evidence_store::ScanReportSection::ConflictSets
            }
            ScanReportSectionDto::NeedsAttention => {
                crate::seams::scan_evidence_store::ScanReportSection::NeedsAttention
            }
            ScanReportSectionDto::Excluded => {
                crate::seams::scan_evidence_store::ScanReportSection::Excluded
            }
        }
    }
}

impl From<crate::seams::scan_evidence_store::ScanReportSection> for ScanReportSectionDto {
    fn from(value: crate::seams::scan_evidence_store::ScanReportSection) -> Self {
        match value {
            crate::seams::scan_evidence_store::ScanReportSection::Roots => Self::Roots,
            crate::seams::scan_evidence_store::ScanReportSection::Entities => Self::Entities,
            crate::seams::scan_evidence_store::ScanReportSection::Appearances => Self::Appearances,
            crate::seams::scan_evidence_store::ScanReportSection::Diagnostics => Self::Diagnostics,
            crate::seams::scan_evidence_store::ScanReportSection::GitSources => Self::GitSources,
            crate::seams::scan_evidence_store::ScanReportSection::LocalCandidates => {
                Self::LocalCandidates
            }
            crate::seams::scan_evidence_store::ScanReportSection::ConflictSets => {
                Self::ConflictSets
            }
            crate::seams::scan_evidence_store::ScanReportSection::NeedsAttention => {
                Self::NeedsAttention
            }
            crate::seams::scan_evidence_store::ScanReportSection::Excluded => Self::Excluded,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReportCursorDto {
    pub report_content_identity: String,
    pub run_id: String,
    pub generation: u64,
    pub section: ScanReportSectionDto,
    pub offset: u64,
}

/// `get_scan_report_page` request: closed cursor + bounded row limit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReportPageRequestDto {
    pub cursor: ScanReportCursorDto,
    /// Rows to return; the native side clamps to 1..=512.
    pub limit: Option<u32>,
}

/// A bounded page of the current Report (spec §4.10): every page carries
/// the pinned Report identity plus the continuation offset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReportPageDto {
    pub report_content_identity: String,
    pub run_id: String,
    pub generation: u64,
    pub section: ScanReportSectionDto,
    pub rows: Vec<ScanReportRowDto>,
    pub next_offset: Option<u64>,
}

/// `start_rescan` request: the trigger is a closed contract value
/// (`onboarding | manual`), never a message.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRescanRequestDto {
    pub trigger: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRescanRequestDto {
    pub run_id: String,
}

impl From<ObservationAndScanSnapshot> for ObservationAndScanSnapshotDto {
    fn from(value: ObservationAndScanSnapshot) -> Self {
        Self {
            home_id: value.home_id,
            write_gate_generation: value.write_gate_generation,
            agent_configuration_generation: value.agent_configuration_generation,
            detection: value.detection.into(),
            startup_probe: value.startup_probe.map(Into::into),
            activation_health: value.activation_health.map(Into::into),
            scan_run: value.scan_run.map(Into::into),
            current_report: value.current_report.into(),
        }
    }
}

impl From<crate::core::scan::ScanRunSnapshot> for ScanRunSnapshotDto {
    fn from(value: crate::core::scan::ScanRunSnapshot) -> Self {
        Self {
            run_id: value.run_id,
            generation: value.generation,
            trigger: value.trigger.as_str().to_owned(),
            state: scan_run_state_name(value.state).to_owned(),
            phase: scan_phase_name(value.phase).to_owned(),
            current_root: value.current_root,
            counts: value.counts.into(),
            roots: value
                .roots
                .into_iter()
                .map(|root| ScanRootViewDto {
                    index: root.index,
                    configured_path: root.configured_path.to_string_lossy().into_owned(),
                    canonical_path: root.canonical_path.to_string_lossy().into_owned(),
                    state: scan_root_view_state_name(root.state).to_owned(),
                    counts: root.counts.into(),
                    elapsed_ms: root.elapsed_ms,
                    slow: root.slow,
                    diagnostic: root.diagnostic,
                })
                .collect(),
            elapsed_ms: value.elapsed_ms,
            slow: value.slow,
            diagnostic: value.diagnostic,
        }
    }
}

impl From<crate::seams::scan_evidence_store::ScanEvidenceCounts> for ScanCountsDto {
    fn from(value: crate::seams::scan_evidence_store::ScanEvidenceCounts) -> Self {
        Self {
            roots: value.roots,
            entries: value.entries,
            entities: value.entities,
            files: value.files,
            bytes: value.bytes,
            git_probes: value.git_probes,
            failed_roots: value.failed_roots,
            configured_agents: value.configured_agents,
            declared_roots: value.declared_roots,
            canonical_roots: value.canonical_roots,
        }
    }
}

impl From<crate::core::scan::CurrentReportView> for CurrentReportDto {
    fn from(value: crate::core::scan::CurrentReportView) -> Self {
        Self {
            summary: value.summary.map(|summary| ScanReportSummaryDto {
                generation: summary.generation,
                run_id: summary.run_id,
                content_identity: summary.content_identity,
                trigger: summary.trigger.as_str().to_owned(),
                state: scan_report_state_name(summary.state).to_owned(),
                coverage: ScanCoverageCountsDto {
                    completed: summary.coverage.completed,
                    failed: summary.coverage.failed,
                    unresponsive: summary.coverage.unresponsive,
                },
                counts: summary.counts.into(),
                incomplete: summary.incomplete,
                published_at_ms: summary.published_at_ms,
                agent_configuration_generation: summary.agent_configuration_generation,
                configured_root_snapshot_fingerprint: summary.configured_root_snapshot_fingerprint,
                started_at_ms: summary.started_at_ms,
                slow: summary.slow,
                source_counts: ScanSourceCountsDto {
                    git_groups: summary.source_counts.git_groups,
                    git_groups_conflicted: summary.source_counts.git_groups_conflicted,
                    local_candidates: summary.source_counts.local_candidates,
                    conflict_sets: summary.source_counts.conflict_sets,
                    conflict_members: summary.source_counts.conflict_members,
                    blocked: summary.source_counts.blocked,
                    deferred: summary.source_counts.deferred,
                    identity_conflicts: summary.source_counts.identity_conflicts,
                    already_managed: summary.source_counts.already_managed,
                    excluded: summary.source_counts.excluded,
                    needs_attention: summary.source_counts.needs_attention,
                },
            }),
            freshness: match value.freshness {
                crate::core::scan::ReportFreshness::Current => ReportFreshnessDto::Current,
                crate::core::scan::ReportFreshness::Stale => ReportFreshnessDto::Stale,
            },
            stale_reasons: value
                .stale_reasons
                .into_iter()
                .map(|reason| match reason {
                    crate::core::scan::StaleReason::CrossStartup => StaleReasonDto::CrossStartup,
                    crate::core::scan::StaleReason::ConfigurationChanged => {
                        StaleReasonDto::ConfigurationChanged
                    }
                    crate::core::scan::StaleReason::HomeOrGateChanged => {
                        StaleReasonDto::HomeOrGateChanged
                    }
                    crate::core::scan::StaleReason::FilesystemChanged => {
                        StaleReasonDto::FilesystemChanged
                    }
                    crate::core::scan::StaleReason::CacheUnreadable => {
                        StaleReasonDto::CacheUnreadable
                    }
                })
                .collect(),
        }
    }
}

fn scan_run_state_name(state: crate::core::scan::ScanRunState) -> &'static str {
    match state {
        crate::core::scan::ScanRunState::Queued => "queued",
        crate::core::scan::ScanRunState::Running => "running",
        crate::core::scan::ScanRunState::Cancelling => "cancelling",
        crate::core::scan::ScanRunState::Cancelled => "cancelled",
        crate::core::scan::ScanRunState::Superseded => "superseded",
        crate::core::scan::ScanRunState::Completed => "completed",
        crate::core::scan::ScanRunState::Failed => "failed",
    }
}

/// Build the public page from the store read: rows are mapped to the typed
/// tagged DTO and the cursor identity is echoed verbatim.
pub fn scan_report_page_dto(
    cursor: &crate::seams::scan_evidence_store::ScanReportCursor,
    page: crate::seams::scan_evidence_store::ScanReportPageRead,
) -> ScanReportPageDto {
    ScanReportPageDto {
        report_content_identity: cursor.report_content_identity.clone(),
        run_id: cursor.run_id.clone(),
        generation: cursor.generation,
        section: ScanReportSectionDto::from(cursor.section),
        rows: page
            .rows
            .into_iter()
            .map(|row| match row {
                crate::seams::scan_evidence_store::ScanReportRow::RootCoverage(root) => {
                    ScanReportRowDto::RootCoverage {
                        index: root.index,
                        configured_path: root.configured_path.to_string_lossy().into_owned(),
                        canonical_path: root.canonical_path.to_string_lossy().into_owned(),
                        state: scan_root_state_name(root.state).to_owned(),
                        consumer_agents: root
                            .consumer_agents
                            .into_iter()
                            .map(|agent| ScanRootAgentDto {
                                agent_id: agent.agent_id,
                                agent_name: agent.agent_name,
                            })
                            .collect(),
                        counts: root.counts.into(),
                        elapsed_ms: root.elapsed_ms,
                        slow: root.slow,
                        diagnostic: root.diagnostic,
                    }
                }
                crate::seams::scan_evidence_store::ScanReportRow::Entity(entity) => {
                    ScanReportRowDto::Entity {
                        entity_seq: entity.entity_seq,
                        identity: ScanObjectIdentityDto {
                            device: entity.identity.device,
                            inode: entity.identity.inode,
                        },
                        canonical_path: entity.canonical_path.to_string_lossy().into_owned(),
                        file_count: entity.file_count,
                        byte_count: entity.byte_count,
                        tree_hash: entity.tree_hash,
                        hash_fault: entity.hash_fault,
                        appearances: entity.appearances,
                        first_root_index: entity.first_root_index,
                        first_entry_seq: entity.first_entry_seq,
                    }
                }
                crate::seams::scan_evidence_store::ScanReportRow::Appearance(appearance) => {
                    ScanReportRowDto::Appearance {
                        root_index: appearance.root_index,
                        seq: appearance.seq,
                        name: appearance.name,
                        entry_path: appearance.entry_path.to_string_lossy().into_owned(),
                        entry_kind: appearance.entry_kind,
                        chain: appearance
                            .chain
                            .into_iter()
                            .map(|hop| ScanChainHopDto {
                                path: hop.path.to_string_lossy().into_owned(),
                                kind: hop.kind,
                                device: hop.device,
                                inode: hop.inode,
                                target: hop
                                    .target
                                    .map(|target| target.to_string_lossy().into_owned()),
                            })
                            .collect(),
                        chain_fault: appearance.chain_fault.map(|fault| ScanChainFaultDto {
                            kind: fault.kind,
                            at: fault.at.to_string_lossy().into_owned(),
                            detail: fault.detail,
                        }),
                        final_entity: appearance
                            .final_entity
                            .map(|path| path.to_string_lossy().into_owned()),
                        identity: appearance.identity.map(|identity| ScanObjectIdentityDto {
                            device: identity.device,
                            inode: identity.inode,
                        }),
                        entity_seq: appearance.entity_seq,
                        lock_hint: Box::new(appearance.lock_hint.map(|hint| ScanLockHintDto {
                            lock_path: hint.lock_path.to_string_lossy().into_owned(),
                            entry_name: hint.entry_name,
                            fingerprint: hint.fingerprint,
                            faulted: hint.faulted,
                            fault: hint.fault,
                            source_type: hint.source_type,
                            source_url: hint.source_url,
                            requested_ref: hint.requested_ref,
                            skill_path: hint.skill_path,
                        })),
                        worktree_hint: appearance.worktree_hint.map(|hint| ScanWorktreeHintDto {
                            repository_root: hint.repository_root.to_string_lossy().into_owned(),
                            gitdir_kind: hint.gitdir_kind,
                            remote_urls: hint.remote_urls,
                            head_ref: hint.head_ref,
                        }),
                    }
                }
                crate::seams::scan_evidence_store::ScanReportRow::GitSourceGroup(group) => {
                    ScanReportRowDto::GitSourceGroup {
                        group_seq: group.group_seq,
                        provider: group.provider,
                        canonical_repository: group.canonical_repository,
                        repository_root: group
                            .repository_root
                            .map(|path| path.to_string_lossy().into_owned()),
                        remote_urls_seen: group.remote_urls_seen,
                        member_entity_seqs: group.member_entity_seqs,
                        member_paths: group
                            .member_paths
                            .into_iter()
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect(),
                        member_names: group.member_names,
                        lock_claims: group
                            .lock_claims
                            .into_iter()
                            .map(scan_lock_claim_dto)
                            .collect(),
                        refs: group.refs,
                        lock_paths: group
                            .lock_paths
                            .into_iter()
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect(),
                        status: group.status,
                        operations: group
                            .operations
                            .into_iter()
                            .map(scan_operation_eligibility_dto)
                            .collect(),
                        detail: group.detail,
                    }
                }
                crate::seams::scan_evidence_store::ScanReportRow::SourceVerdict(verdict) => {
                    ScanReportRowDto::SourceVerdict {
                        entity_seq: verdict.entity_seq,
                        verdict: verdict.verdict,
                        canonical_path: verdict.canonical_path.to_string_lossy().into_owned(),
                        directory_names: verdict.directory_names,
                        appearances: verdict.appearances,
                        file_count: verdict.file_count,
                        byte_count: verdict.byte_count,
                        tree_hash: verdict.tree_hash,
                        lock_claims: verdict
                            .lock_claims
                            .into_iter()
                            .map(scan_lock_claim_dto)
                            .collect(),
                        worktree_hints: verdict
                            .worktree_hints
                            .into_iter()
                            .map(|hint| ScanWorktreeHintDto {
                                repository_root: hint.repository_root.to_string_lossy().into_owned(),
                                gitdir_kind: hint.gitdir_kind,
                                remote_urls: hint.remote_urls,
                                head_ref: hint.head_ref,
                            })
                            .collect(),
                        reason_kind: verdict.reason_kind,
                        detail: verdict.detail,
                        git_refs: verdict.git_refs,
                        git_lock_paths: verdict
                            .git_lock_paths
                            .into_iter()
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect(),
                        git_group_seq: verdict.git_group_seq,
                        conflict_set_seq: verdict.conflict_set_seq,
                        notes: verdict.notes,
                        operations: verdict
                            .operations
                            .into_iter()
                            .map(scan_operation_eligibility_dto)
                            .collect(),
                    }
                }
                crate::seams::scan_evidence_store::ScanReportRow::ConflictSet(set) => {
                    ScanReportRowDto::ConflictSet {
                        set_seq: set.set_seq,
                        directory_identity_key: set.directory_identity_key,
                        directory_name: set.directory_name,
                        member_entity_seqs: set.member_entity_seqs,
                        member_paths: set
                            .member_paths
                            .into_iter()
                            .map(|path| path.to_string_lossy().into_owned())
                            .collect(),
                        winner_entity_seq: set.winner_entity_seq,
                    }
                }
                crate::seams::scan_evidence_store::ScanReportRow::Diagnostic(diagnostic) => {
                    ScanReportRowDto::Diagnostic {
                        root_index: diagnostic.root_index,
                        diagnostic_kind: diagnostic.kind,
                        at: diagnostic
                            .at
                            .map(|path| path.to_string_lossy().into_owned()),
                        detail: diagnostic.detail,
                    }
                }
            })
            .collect(),
        next_offset: page.next_offset,
    }
}

fn scan_lock_claim_dto(
    claim: crate::seams::scan_evidence_store::ScanLockClaimRecord,
) -> ScanLockClaimDto {
    ScanLockClaimDto {
        lock_path: claim.lock_path.to_string_lossy().into_owned(),
        entry_name: claim.entry_name,
        fingerprint: claim.fingerprint,
        source_type: claim.source_type,
        source_url: claim.source_url,
        requested_ref: claim.requested_ref,
        skill_path: claim.skill_path,
    }
}

fn scan_operation_eligibility_dto(
    eligibility: crate::seams::scan_evidence_store::ScanOperationEligibility,
) -> ScanOperationEligibilityDto {
    ScanOperationEligibilityDto {
        operation: eligibility.operation,
        allowed: eligibility.allowed,
        closed_reason: eligibility.closed_reason,
    }
}

fn scan_root_state_name(state: crate::seams::scan_evidence_store::ScanRootState) -> &'static str {
    match state {
        crate::seams::scan_evidence_store::ScanRootState::Completed => "completed",
        crate::seams::scan_evidence_store::ScanRootState::Failed => "failed",
        crate::seams::scan_evidence_store::ScanRootState::Unresponsive => "unresponsive",
    }
}

fn scan_phase_name(phase: crate::core::scan::ScanPhase) -> &'static str {
    match phase {
        crate::core::scan::ScanPhase::Planning => "planning",
        crate::core::scan::ScanPhase::Walking => "walking",
        crate::core::scan::ScanPhase::Hashing => "hashing",
        crate::core::scan::ScanPhase::Finalizing => "finalizing",
    }
}

fn scan_root_view_state_name(state: crate::core::scan::ScanRootViewState) -> &'static str {
    match state {
        crate::core::scan::ScanRootViewState::Pending => "pending",
        crate::core::scan::ScanRootViewState::Walking => "walking",
        crate::core::scan::ScanRootViewState::Completed => "completed",
        crate::core::scan::ScanRootViewState::Failed => "failed",
        crate::core::scan::ScanRootViewState::Unresponsive => "unresponsive",
    }
}

fn scan_report_state_name(state: crate::core::scan::ScanReportState) -> &'static str {
    match state {
        crate::core::scan::ScanReportState::Complete => "complete",
        crate::core::scan::ScanReportState::Incomplete => "incomplete",
    }
}

fn root_detection_state_name(state: crate::core::observation::RootDetectionState) -> &'static str {
    match state {
        crate::core::observation::RootDetectionState::Present => "present",
        crate::core::observation::RootDetectionState::Unavailable => "unavailable",
        crate::core::observation::RootDetectionState::Absent => "absent",
    }
}

fn activation_observed_state_name(state: ActivationObservedState) -> &'static str {
    match state {
        ActivationObservedState::Present => "present",
        ActivationObservedState::Missing => "missing",
        ActivationObservedState::TargetMismatch => "target_mismatch",
        ActivationObservedState::Dangling => "dangling",
        ActivationObservedState::Occupied => "occupied",
    }
}

/// The unique paged observation read request (spec §4.10
/// `observation_page`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ObservationPageRequestDto {
    /// `startup_probe | activation_health`.
    pub kind: ObservationKindDto,
    pub generation: u64,
    pub cursor: ObservationCursorDto,
    pub limit: Option<usize>,
}

/// Target-scoped Activation Health trigger (spec §4.10): `None` = all
/// Targets, otherwise only the affected Target Root ids are re-observed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RefreshActivationHealthRequestDto {
    pub target_root_ids: Option<Vec<String>>,
}

/// A bounded page converted from the core page read; the kind-specific row
/// payload is kept lossless on the wire with a tagged `kind` + `row`.
pub fn observation_page_read_dto(
    _kind: ObservationKind,
    generation: u64,
    page: crate::core::observation::ObservationPageRead,
) -> ObservationPageReadDto {
    ObservationPageReadDto {
        generation,
        rows: page
            .rows
            .into_iter()
            .map(|row| match row {
                crate::core::observation::ObservationRow::StartupProbe(row) => {
                    ObservationRowDto::StartupProbe { row: row.into() }
                }
                crate::core::observation::ObservationRow::ActivationHealth(row) => {
                    ObservationRowDto::ActivationHealth { row: row.into() }
                }
            })
            .collect(),
        next_offset: page.next_offset,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKindDto {
    ClaudePreset,
    CodexPreset,
    Custom,
}

impl From<AgentKind> for AgentKindDto {
    fn from(value: AgentKind) -> Self {
        match value {
            AgentKind::ClaudePreset => Self::ClaudePreset,
            AgentKind::CodexPreset => Self::CodexPreset,
            AgentKind::Custom => Self::Custom,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityDto {
    Verified,
    Unknown,
}

impl From<Compatibility> for CompatibilityDto {
    fn from(value: Compatibility) -> Self {
        match value {
            Compatibility::Verified => Self::Verified,
            Compatibility::Unknown => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationHealthReportDto {
    pub checked: u32,
    pub snapshot_version: u64,
}

impl From<ActivationHealthReport> for ActivationHealthReportDto {
    fn from(value: ActivationHealthReport) -> Self {
        Self {
            checked: value.checked,
            snapshot_version: value.snapshot_version,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelocateLinkRequestDto {
    pub skill_id: String,
    pub source_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelocateLinkPreviewDto {
    pub plan_token: String,
    pub skill_id: String,
    pub directory_name: String,
    pub source_entry_path: String,
    pub final_entity_path: String,
    pub display_name: String,
    pub description: String,
    pub frontmatter_name: Option<String>,
    pub activation_count: u32,
}

impl From<RelocatePreview> for RelocateLinkPreviewDto {
    fn from(value: RelocatePreview) -> Self {
        Self {
            plan_token: value.plan_token,
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            source_entry_path: value.source_entry_path.to_string_lossy().into_owned(),
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            display_name: value.display_name,
            description: value.description,
            frontmatter_name: value.frontmatter_name,
            activation_count: value.activation_count,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyRelocateLinkRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelocateLinkResultDto {
    pub skill_id: String,
    pub directory_name: String,
    pub final_entity_path: String,
    pub activation_count: u32,
    pub snapshot_version: u64,
}

impl From<RelocateResult> for RelocateLinkResultDto {
    fn from(value: RelocateResult) -> Self {
        Self {
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            activation_count: value.activation_count,
            snapshot_version: value.snapshot_version,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRelocateLinkRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRemoveSkillRequestDto {
    pub skill_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveSkillPreviewDto {
    pub plan_token: String,
    pub skill_id: String,
    pub directory_name: String,
    pub source_kind: SourceKindDto,
    pub final_entity_path: String,
    pub activation_count: u32,
}

impl From<RemovePreview> for RemoveSkillPreviewDto {
    fn from(value: RemovePreview) -> Self {
        Self {
            plan_token: value.plan_token,
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            source_kind: SourceKindDto::from(value.source_kind),
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            activation_count: value.activation_count,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyRemoveSkillRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveSkillResultDto {
    pub skill_id: String,
    pub directory_name: String,
    pub snapshot_version: u64,
}

impl From<RemoveResult> for RemoveSkillResultDto {
    fn from(value: RemoveResult) -> Self {
        Self {
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            snapshot_version: value.snapshot_version,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRemoveSkillRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverLinkImportRequestDto {
    pub source_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanLinkImportRequestDto {
    pub source_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyLinkImportRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelLinkImportRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverFileImportRequestDto {
    pub source_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverFileImportCollectionRequestDto {
    pub source_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFileImportRequestDto {
    pub source_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFileReinstallRequestDto {
    pub source_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFileImportSelectionRequestDto {
    pub source_path: String,
    pub selected_directory_names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyFileImportRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyFileImportSelectionRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelFileImportRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkImportCandidateDto {
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub frontmatter_name: Option<String>,
    pub source_entry_path: String,
    pub final_entity_path: String,
}

impl From<LinkImportCandidate> for LinkImportCandidateDto {
    fn from(value: LinkImportCandidate) -> Self {
        Self {
            directory_name: value.directory_name,
            display_name: value.display_name,
            description: value.description,
            frontmatter_name: value.frontmatter_name,
            source_entry_path: value.source_entry_path.to_string_lossy().into_owned(),
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileImportCandidateDto {
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub frontmatter_name: Option<String>,
    pub original_path: String,
    pub original_filename: String,
}

impl From<FileImportCandidate> for FileImportCandidateDto {
    fn from(value: FileImportCandidate) -> Self {
        Self {
            directory_name: value.directory_name,
            display_name: value.display_name,
            description: value.description,
            frontmatter_name: value.frontmatter_name,
            original_path: value.original_path.to_string_lossy().into_owned(),
            original_filename: value.original_filename,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileImportDiscoveryDto {
    pub candidates: Vec<FileImportCandidateDto>,
    pub truncated: bool,
}

impl From<FileImportDiscovery> for FileImportDiscoveryDto {
    fn from(value: FileImportDiscovery) -> Self {
        Self {
            candidates: value
                .candidates
                .into_iter()
                .map(FileImportCandidateDto::from)
                .collect(),
            truncated: value.truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryConflictDto {
    pub existing_skill_id: String,
    pub directory_name: String,
}

impl From<LibraryConflict> for LibraryConflictDto {
    fn from(value: LibraryConflict) -> Self {
        Self {
            existing_skill_id: value.existing_skill_id.0,
            directory_name: value.directory_name,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkImportPreviewDto {
    pub plan_token: String,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub source_entry_path: String,
    pub final_entity_path: String,
    pub library_entry_path: Option<String>,
    pub conflict: Option<LibraryConflictDto>,
    pub can_apply: bool,
}

impl From<LinkImportPreview> for LinkImportPreviewDto {
    fn from(value: LinkImportPreview) -> Self {
        Self {
            plan_token: value.plan_token,
            directory_name: value.directory_name,
            display_name: value.display_name,
            description: value.description,
            source_entry_path: value.source_entry_path.to_string_lossy().into_owned(),
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            library_entry_path: None,
            conflict: value.conflict.map(LibraryConflictDto::from),
            can_apply: value.can_apply,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileImportPreviewDto {
    pub plan_token: String,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub original_path: String,
    pub original_filename: String,
    pub final_entity_path: String,
    pub conflict: Option<LibraryConflictDto>,
    pub can_apply: bool,
}

impl From<FileImportPreview> for FileImportPreviewDto {
    fn from(value: FileImportPreview) -> Self {
        Self {
            plan_token: value.plan_token,
            directory_name: value.directory_name,
            display_name: value.display_name,
            description: value.description,
            original_path: value.original_path.to_string_lossy().into_owned(),
            original_filename: value.original_filename,
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            conflict: value.conflict.map(LibraryConflictDto::from),
            can_apply: value.can_apply,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileImportSelectionPreviewDto {
    pub plan_token: String,
    pub items: Vec<FileImportPreviewDto>,
    pub can_apply: bool,
}

impl From<FileImportSelectionPreview> for FileImportSelectionPreviewDto {
    fn from(value: FileImportSelectionPreview) -> Self {
        Self {
            plan_token: value.plan_token,
            items: value
                .items
                .into_iter()
                .map(FileImportPreviewDto::from)
                .collect(),
            can_apply: value.can_apply,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkImportResultDto {
    pub operation_id: String,
    pub skill_id: String,
    pub directory_name: String,
    pub final_entity_path: String,
    pub library_entry_path: Option<String>,
    pub snapshot_version: u64,
}

impl From<LinkImportResult> for LinkImportResultDto {
    fn from(value: LinkImportResult) -> Self {
        Self {
            operation_id: value.operation_id,
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            library_entry_path: None,
            snapshot_version: value.snapshot_version,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileImportResultDto {
    pub operation_id: String,
    pub skill_id: String,
    pub directory_name: String,
    pub final_entity_path: String,
    pub library_entry_path: String,
    pub snapshot_version: u64,
    pub created_paths: Vec<String>,
    pub removed_paths: Vec<String>,
    pub retained_paths: Vec<String>,
    pub retryable: bool,
    pub recovery_required: bool,
}

impl From<FileImportResult> for FileImportResultDto {
    fn from(value: FileImportResult) -> Self {
        let stable_path = value.final_entity_path.to_string_lossy().into_owned();
        let (created_paths, retained_paths) = if value.reinstalled {
            (Vec::new(), vec![stable_path.clone()])
        } else {
            (vec![stable_path.clone()], Vec::new())
        };
        Self {
            operation_id: value.operation_id,
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            library_entry_path: value.final_entity_path.to_string_lossy().into_owned(),
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            snapshot_version: value.snapshot_version,
            created_paths,
            removed_paths: Vec::new(),
            retained_paths,
            retryable: false,
            recovery_required: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileImportSelectionResultDto {
    pub operation_id: String,
    pub items: Vec<FileImportResultDto>,
    pub snapshot_version: u64,
    pub retryable: bool,
    pub recovery_required: bool,
}

impl From<FileImportSelectionResult> for FileImportSelectionResultDto {
    fn from(value: FileImportSelectionResult) -> Self {
        Self {
            operation_id: value.operation_id,
            items: value
                .items
                .into_iter()
                .map(FileImportResultDto::from)
                .collect(),
            snapshot_version: value.snapshot_version,
            retryable: false,
            recovery_required: false,
        }
    }
}

// -- Git remote Import --

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverGitImportRequestDto {
    pub source: String,
    pub force_full_depth: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanGitImportSelectionRequestDto {
    pub source: String,
    pub force_full_depth: bool,
    pub selected_directory_names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyGitImportSelectionRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelGitImportSelectionRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitImportCandidateDto {
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub frontmatter_name: Option<String>,
    pub skill_path: String,
}

impl From<GitImportCandidate> for GitImportCandidateDto {
    fn from(value: GitImportCandidate) -> Self {
        Self {
            directory_name: value.directory_name,
            display_name: value.display_name,
            description: value.description,
            frontmatter_name: value.frontmatter_name,
            skill_path: value.skill_path,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitImportDiscoveryDto {
    pub repo_url: String,
    pub requested_ref: String,
    pub resolved_commit: String,
    pub candidates: Vec<GitImportCandidateDto>,
    pub truncated: bool,
}

impl From<GitImportDiscovery> for GitImportDiscoveryDto {
    fn from(value: GitImportDiscovery) -> Self {
        Self {
            repo_url: value.repo_url,
            requested_ref: value.requested_ref,
            resolved_commit: value.resolved_commit,
            candidates: value
                .candidates
                .into_iter()
                .map(GitImportCandidateDto::from)
                .collect(),
            truncated: value.truncated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitImportPreviewDto {
    pub plan_token: String,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub skill_path: String,
    pub final_entity_path: String,
    pub conflict: Option<LibraryConflictDto>,
    pub can_apply: bool,
}

impl From<GitImportPreview> for GitImportPreviewDto {
    fn from(value: GitImportPreview) -> Self {
        Self {
            plan_token: value.plan_token,
            directory_name: value.directory_name,
            display_name: value.display_name,
            description: value.description,
            skill_path: value.skill_path,
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            conflict: value.conflict.map(LibraryConflictDto::from),
            can_apply: value.can_apply,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitImportSelectionPreviewDto {
    pub plan_token: String,
    pub repo_url: String,
    pub requested_ref: String,
    pub resolved_commit: String,
    pub items: Vec<GitImportPreviewDto>,
    pub can_apply: bool,
}

impl From<GitImportSelectionPreview> for GitImportSelectionPreviewDto {
    fn from(value: GitImportSelectionPreview) -> Self {
        Self {
            plan_token: value.plan_token,
            repo_url: value.repo_url,
            requested_ref: value.requested_ref,
            resolved_commit: value.resolved_commit,
            items: value
                .items
                .into_iter()
                .map(GitImportPreviewDto::from)
                .collect(),
            can_apply: value.can_apply,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitImportResultDto {
    pub operation_id: String,
    pub skill_id: String,
    pub directory_name: String,
    pub final_entity_path: String,
    pub snapshot_version: u64,
}

impl From<GitImportResult> for GitImportResultDto {
    fn from(value: GitImportResult) -> Self {
        Self {
            operation_id: value.operation_id,
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            snapshot_version: value.snapshot_version,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitImportSelectionResultDto {
    pub operation_id: String,
    pub items: Vec<GitImportResultDto>,
    pub snapshot_version: u64,
}

impl From<GitImportSelectionResult> for GitImportSelectionResultDto {
    fn from(value: GitImportSelectionResult) -> Self {
        Self {
            operation_id: value.operation_id,
            items: value
                .items
                .into_iter()
                .map(GitImportResultDto::from)
                .collect(),
            snapshot_version: value.snapshot_version,
        }
    }
}

// -- Skill Updates --

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckSkillUpdatesRequestDto {
    pub force: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckItemDto {
    pub skill_id: String,
    pub directory_name: String,
    pub source_url: String,
    pub requested_ref: String,
    pub current_commit: String,
    pub resolved_commit: String,
    pub has_update: bool,
    pub modified: bool,
    pub upstream_path_gone: bool,
    pub last_checked_at: Option<String>,
}

impl From<UpdateCheckItem> for UpdateCheckItemDto {
    fn from(value: UpdateCheckItem) -> Self {
        Self {
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            source_url: value.source_url,
            requested_ref: value.requested_ref,
            current_commit: value.current_commit,
            resolved_commit: value.resolved_commit,
            has_update: value.has_update,
            modified: value.modified,
            upstream_path_gone: value.upstream_path_gone,
            last_checked_at: value.last_checked_at.map(epoch_seconds_to_rfc3339),
        }
    }
}

// Spec §5.3: persisted times may be integer epoch, DTOs always output
// RFC 3339. The formatter lives in the recovery core (single source of
// truth shared by the ledger and the DTO layer).
use crate::core::fixture_recovery::epoch_seconds_to_rfc3339;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckGroupDto {
    pub repo_url: String,
    pub items: Vec<UpdateCheckItemDto>,
}

impl From<UpdateCheckGroup> for UpdateCheckGroupDto {
    fn from(value: UpdateCheckGroup) -> Self {
        Self {
            repo_url: value.repo_url,
            items: value
                .items
                .into_iter()
                .map(UpdateCheckItemDto::from)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckReportDto {
    pub groups: Vec<UpdateCheckGroupDto>,
    /// Repo-scoped failures (offline, resolution, listing).
    pub errors: Vec<String>,
    /// Closed Remote Source Identity Conflict entries (ADR-0013 §4.3): the
    /// parent manifest disagrees with the Catalog row, so Update for that
    /// parent is closed. Read/Disable/Remove and other parents continue.
    pub parent_conflicts: Vec<ParentConflictDto>,
}

/// One parent whose manifest/row mismatch closes its Update (spec §4.7:
/// closed code plus typed params; the URL is Source Content).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentConflictDto {
    pub remote_id: String,
    pub canonical_url: String,
}

impl From<UpdateCheckReport> for UpdateCheckReportDto {
    fn from(value: UpdateCheckReport) -> Self {
        Self {
            groups: value
                .groups
                .into_iter()
                .map(UpdateCheckGroupDto::from)
                .collect(),
            errors: value.errors,
            parent_conflicts: value
                .parent_conflicts
                .into_iter()
                .map(|conflict| ParentConflictDto {
                    remote_id: conflict.remote_id,
                    canonical_url: conflict.canonical_url,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSelectionDto {
    pub skill_id: String,
    pub new_skill_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSkillUpdatesRequestDto {
    pub selections: Vec<UpdateSelectionDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlanItemDto {
    pub skill_id: String,
    pub directory_name: String,
    pub plan_token: String,
    pub current_commit: String,
    pub new_commit: String,
    pub modified: bool,
    pub path_changed: bool,
    pub error: Option<String>,
}

impl From<UpdatePlanItem> for UpdatePlanItemDto {
    fn from(value: UpdatePlanItem) -> Self {
        Self {
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            plan_token: value.plan_token,
            current_commit: value.current_commit,
            new_commit: value.new_commit,
            modified: value.modified,
            path_changed: value.path_changed,
            error: value.error,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlanDto {
    pub items: Vec<UpdatePlanItemDto>,
}

impl From<UpdatePlan> for UpdatePlanDto {
    fn from(value: UpdatePlan) -> Self {
        Self {
            items: value
                .items
                .into_iter()
                .map(UpdatePlanItemDto::from)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateApplyRequestDto {
    pub plan_token: String,
    pub skill_id: String,
    pub directory_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplySkillUpdatesRequestDto {
    pub requests: Vec<UpdateApplyRequestDto>,
    pub abandon_changes: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateItemResultDto {
    pub skill_id: String,
    pub directory_name: String,
    pub updated: bool,
    pub error: Option<String>,
}

impl From<UpdateItemResult> for UpdateItemResultDto {
    fn from(value: UpdateItemResult) -> Self {
        Self {
            skill_id: value.skill_id.0,
            directory_name: value.directory_name,
            updated: value.updated,
            error: value.error,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResultDto {
    pub items: Vec<UpdateItemResultDto>,
}

impl From<UpdateResult> for UpdateResultDto {
    fn from(value: UpdateResult) -> Self {
        Self {
            items: value
                .items
                .into_iter()
                .map(UpdateItemResultDto::from)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinSkillUpdatesRequestDto {
    pub skill_ids: Vec<String>,
}

// -- Adopt --

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptVerdictDto {
    Local,
    Verified,
    Modified,
    Conflict,
    Deferred,
    Blocked,
    Excluded,
}

impl From<crate::core::adopt::AdoptVerdict> for AdoptVerdictDto {
    fn from(value: crate::core::adopt::AdoptVerdict) -> Self {
        use crate::core::adopt::AdoptVerdict as Verdict;
        match value {
            Verdict::Local => Self::Local,
            Verdict::Verified => Self::Verified,
            Verdict::Modified => Self::Modified,
            Verdict::Conflict => Self::Conflict,
            Verdict::Deferred => Self::Deferred,
            Verdict::Blocked => Self::Blocked,
            Verdict::Excluded => Self::Excluded,
        }
    }
}

/// Closed lock-file faults (spec §8.1); reasons are raw facts, never App
/// Copy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum LockFileFaultDto {
    NotUtf8,
    InvalidJson { detail: String },
    UnsupportedVersion { version: u64 },
    DuplicateKey { key: String },
}

impl From<crate::seams::installer_lock_store::LockFileFault> for LockFileFaultDto {
    fn from(value: crate::seams::installer_lock_store::LockFileFault) -> Self {
        use crate::seams::installer_lock_store::LockFileFault as Fault;
        match value {
            Fault::NotUtf8 => Self::NotUtf8,
            Fault::InvalidJson(detail) => Self::InvalidJson { detail },
            Fault::UnsupportedVersion(version) => Self::UnsupportedVersion { version },
            Fault::DuplicateKey(key) => Self::DuplicateKey { key },
        }
    }
}

/// Closed chain-fault reasons (spec §8.1); a failure stops at the exact hop
/// and never yields a partial fingerprint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ChainFaultDto {
    Dangling { at: String },
    Cycle { at: String },
    HopLimit { at: String },
    NonUtf8 { at: String },
    ReadFailed { at: String, detail: String },
    NotDirectory { at: String },
    IdentityReplaced { at: String },
}

impl From<crate::seams::filesystem::ChainFault> for ChainFaultDto {
    fn from(value: crate::seams::filesystem::ChainFault) -> Self {
        use crate::seams::filesystem::ChainFault as Fault;
        let display = |path: std::path::PathBuf| path.to_string_lossy().into_owned();
        match value {
            Fault::Dangling { at } => Self::Dangling { at: display(at) },
            Fault::Cycle { at } => Self::Cycle { at: display(at) },
            Fault::HopLimit { at } => Self::HopLimit { at: display(at) },
            Fault::NonUtf8 { at } => Self::NonUtf8 { at: display(at) },
            Fault::ReadFailed { at, detail } => Self::ReadFailed {
                at: display(at),
                detail,
            },
            Fault::NotDirectory { at } => Self::NotDirectory { at: display(at) },
            Fault::IdentityReplaced { at } => Self::IdentityReplaced { at: display(at) },
        }
    }
}

/// The typed reason behind a verdict (spec §4.7): closed states, never
/// free-form warning strings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AdoptVerdictReasonDto {
    NoLock,
    DuplicateLockOwner {
        other_lock_path: String,
    },
    LockFileFault {
        lock_path: String,
        fault: LockFileFaultDto,
    },
    LockEntryFault {
        lock_path: String,
        reason: String,
    },
    EntityNotAtInstallerRoot {
        expected: String,
    },
    RemoteConflict {
        detail: String,
    },
    IdentityConflict {
        names: Vec<String>,
    },
    LibraryConflict {
        directory_name: String,
    },
    OwnershipConflict {
        managed_directory_name: String,
    },
    RemoteUnavailable {
        detail: String,
    },
    ChainFault {
        fault: ChainFaultDto,
    },
    UnreadableEntity {
        detail: String,
    },
    FixtureEntity,
}

impl From<crate::core::adopt::AdoptVerdictReason> for AdoptVerdictReasonDto {
    fn from(value: crate::core::adopt::AdoptVerdictReason) -> Self {
        use crate::core::adopt::AdoptVerdictReason as Reason;
        let display = |path: std::path::PathBuf| path.to_string_lossy().into_owned();
        match value {
            Reason::NoLock => Self::NoLock,
            Reason::DuplicateLockOwner { other_lock_path } => Self::DuplicateLockOwner {
                other_lock_path: display(other_lock_path),
            },
            Reason::LockFileFault { lock_path, fault } => Self::LockFileFault {
                lock_path: display(lock_path),
                fault: fault.into(),
            },
            Reason::LockEntryFault { lock_path, reason } => Self::LockEntryFault {
                lock_path: display(lock_path),
                reason,
            },
            Reason::EntityNotAtInstallerRoot { expected } => Self::EntityNotAtInstallerRoot {
                expected: display(expected),
            },
            Reason::RemoteConflict { detail } => Self::RemoteConflict { detail },
            Reason::IdentityConflict { names } => Self::IdentityConflict { names },
            Reason::LibraryConflict { directory_name } => Self::LibraryConflict { directory_name },
            Reason::OwnershipConflict {
                managed_directory_name,
            } => Self::OwnershipConflict {
                managed_directory_name,
            },
            Reason::RemoteUnavailable { detail } => Self::RemoteUnavailable { detail },
            Reason::ChainFault { fault } => Self::ChainFault {
                fault: fault.into(),
            },
            Reason::UnreadableEntity { detail } => Self::UnreadableEntity { detail },
            Reason::FixtureEntity => Self::FixtureEntity,
        }
    }
}

/// The hop entry kind; the raw symlink target is a separate Source Content
/// field (spec §4.7: closed codes plus typed params, never stringly-typed).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceChainHopKindDto {
    Directory,
    Symlink,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceChainHopDto {
    pub path: String,
    pub kind: EvidenceChainHopKindDto,
    /// The raw symlink target text when the hop is a symlink; `None` for
    /// real directory hops. Source Content, rendered verbatim.
    pub target: Option<String>,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceChainDto {
    pub entry_path: String,
    pub entry_device: u64,
    pub entry_inode: u64,
    pub hops: Vec<EvidenceChainHopDto>,
    pub final_entity: Option<String>,
    pub fault: Option<ChainFaultDto>,
}

impl From<crate::seams::filesystem::EvidenceChain> for EvidenceChainDto {
    fn from(value: crate::seams::filesystem::EvidenceChain) -> Self {
        Self {
            entry_path: value.entry_path.to_string_lossy().into_owned(),
            entry_device: value.entry_device,
            entry_inode: value.entry_inode,
            hops: value
                .hops
                .into_iter()
                .map(|hop| EvidenceChainHopDto {
                    path: hop.path.to_string_lossy().into_owned(),
                    kind: match hop.kind {
                        crate::seams::filesystem::EvidenceChainHopKind::Directory => {
                            EvidenceChainHopKindDto::Directory
                        }
                        crate::seams::filesystem::EvidenceChainHopKind::Symlink { .. } => {
                            EvidenceChainHopKindDto::Symlink
                        }
                    },
                    target: match hop.kind {
                        crate::seams::filesystem::EvidenceChainHopKind::Symlink { target } => {
                            Some(target.to_string_lossy().into_owned())
                        }
                        crate::seams::filesystem::EvidenceChainHopKind::Directory => None,
                    },
                    device: hop.device,
                    inode: hop.inode,
                })
                .collect(),
            final_entity: value
                .final_entity
                .map(|path| path.to_string_lossy().into_owned()),
            fault: value.fault.map(Into::into),
        }
    }
}

/// One Agent appearance with its full source chain (spec §8.1); raw paths
/// and symlink targets are Source Content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptAppearanceEvidenceDto {
    pub entry_path: String,
    pub kind: String,
    pub agent_id: Option<String>,
    pub shared: bool,
    pub original_target: Option<String>,
    pub chain: EvidenceChainDto,
}

impl From<crate::core::adopt::AdoptAppearanceEvidence> for AdoptAppearanceEvidenceDto {
    fn from(value: crate::core::adopt::AdoptAppearanceEvidence) -> Self {
        let original_target = match &value.appearance.kind {
            crate::core::adopt::AdoptAppearanceKind::Symlink { original_target } => {
                Some(original_target.to_string_lossy().into_owned())
            }
            crate::core::adopt::AdoptAppearanceKind::RealDirectory => None,
        };
        Self {
            entry_path: value.appearance.entry_path.to_string_lossy().into_owned(),
            kind: match value.appearance.kind {
                crate::core::adopt::AdoptAppearanceKind::RealDirectory => "real_directory".into(),
                crate::core::adopt::AdoptAppearanceKind::Symlink { .. } => "symlink".into(),
            },
            agent_id: value.appearance.agent_id.map(|agent_id| agent_id.0),
            shared: value.appearance.shared,
            original_target,
            chain: value.chain.into(),
        }
    }
}

/// The strict lock entry as raw Source Content (spec §6.3): fields render
/// verbatim in every locale.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockEntryDto {
    pub name: String,
    pub source_type: String,
    pub source: String,
    pub source_url: String,
    pub requested_ref: Option<String>,
    pub skill_path: String,
    pub skill_folder_hash: String,
    pub installed_at: Option<String>,
    pub updated_at: Option<String>,
    pub plugin_name: Option<String>,
}

impl From<crate::seams::installer_lock_store::LockEntry> for LockEntryDto {
    fn from(value: crate::seams::installer_lock_store::LockEntry) -> Self {
        Self {
            name: value.name,
            source_type: value.source_type,
            source: value.source,
            source_url: value.source_url,
            requested_ref: value.requested_ref,
            skill_path: value.skill_path,
            skill_folder_hash: value.skill_folder_hash,
            installed_at: value.installed_at,
            updated_at: value.updated_at,
            plugin_name: value.plugin_name,
        }
    }
}

/// The lock evidence for one candidate (spec §8.1).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptLockEvidenceDto {
    pub lock_path: String,
    pub lock_fingerprint: String,
    pub entry_name: String,
    pub entry: Option<LockEntryDto>,
    pub entry_fault: Option<String>,
    pub file_fault: Option<LockFileFaultDto>,
}

impl From<crate::core::adopt::AdoptLockEvidence> for AdoptLockEvidenceDto {
    fn from(value: crate::core::adopt::AdoptLockEvidence) -> Self {
        Self {
            lock_path: value.lock_path.to_string_lossy().into_owned(),
            lock_fingerprint: value.lock_fingerprint,
            entry_name: value.entry_name,
            entry: value.entry.map(Into::into),
            entry_fault: value.entry_fault,
            file_fault: value.file_fault.map(Into::into),
        }
    }
}

/// The requested ref disposition (spec §8.1): closed states mirroring
/// `RefDisposition`, never free-form strings (ADR-0011).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKindDto {
    Head,
    Branch,
    Tag,
    Commit,
}

impl From<crate::seams::remote_provider::RefDisposition> for RefKindDto {
    fn from(value: crate::seams::remote_provider::RefDisposition) -> Self {
        match value {
            crate::seams::remote_provider::RefDisposition::Head => RefKindDto::Head,
            crate::seams::remote_provider::RefDisposition::Branch => RefKindDto::Branch,
            crate::seams::remote_provider::RefDisposition::Tag => RefKindDto::Tag,
            crate::seams::remote_provider::RefDisposition::Commit => RefKindDto::Commit,
        }
    }
}

/// The remote side of a closed loop (spec §8.1).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptRemoteEvidenceDto {
    pub canonical_url: String,
    pub requested_ref: String,
    pub ref_kind: RefKindDto,
    pub anchor_commit: String,
    pub original_install_commit_known: bool,
    pub skill_path: String,
    pub provider_hash: String,
    pub provider_hash_matched: bool,
    pub remote_tree_hash: String,
    pub local_tree_hash: String,
    pub trees_match: bool,
    pub default_branch: Option<String>,
}

impl From<crate::core::adopt::AdoptRemoteEvidence> for AdoptRemoteEvidenceDto {
    fn from(value: crate::core::adopt::AdoptRemoteEvidence) -> Self {
        Self {
            canonical_url: value.canonical_url,
            requested_ref: value.requested_ref,
            ref_kind: value.ref_kind.into(),
            anchor_commit: value.anchor_commit,
            original_install_commit_known: value.original_install_commit_known,
            skill_path: value.skill_path,
            provider_hash: value.provider_hash,
            provider_hash_matched: value.provider_hash_matched,
            remote_tree_hash: value.remote_tree_hash,
            local_tree_hash: value.local_tree_hash,
            trees_match: value.trees_match,
            default_branch: value.default_branch,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptEvidenceCandidateDto {
    pub canonical_entity: String,
    pub directory_name: String,
    pub directory_names: Vec<String>,
    pub appearances: Vec<AdoptAppearanceEvidenceDto>,
    pub verdict: AdoptVerdictDto,
    pub reason: Option<AdoptVerdictReasonDto>,
    pub lock: Option<AdoptLockEvidenceDto>,
    pub remote: Option<AdoptRemoteEvidenceDto>,
    pub local_tree_hash: Option<String>,
    pub requires_relocation: bool,
    pub selectable: bool,
    pub adoptable: bool,
    pub conflict: Option<LibraryConflictDto>,
    pub suggested_agent_ids: Vec<String>,
}

impl From<crate::core::adopt::AdoptEvidenceCandidate> for AdoptEvidenceCandidateDto {
    fn from(value: crate::core::adopt::AdoptEvidenceCandidate) -> Self {
        Self {
            canonical_entity: value.canonical_entity.to_string_lossy().into_owned(),
            directory_name: value.directory_name,
            directory_names: value.directory_names,
            appearances: value.appearances.into_iter().map(Into::into).collect(),
            verdict: value.verdict.into(),
            reason: value.reason.map(Into::into),
            lock: value.lock.map(Into::into),
            remote: value.remote.map(Into::into),
            local_tree_hash: value.local_tree_hash,
            requires_relocation: value.requires_relocation,
            selectable: value.selectable,
            adoptable: value.adoptable,
            conflict: value.conflict.map(LibraryConflictDto::from),
            suggested_agent_ids: value
                .suggested_agent_ids
                .into_iter()
                .map(|agent_id| agent_id.0)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptGitSourceClaimDto {
    pub lock_path: String,
    pub entry_name: String,
    pub requested_ref: String,
}

impl From<crate::core::adopt::AdoptGitSourceClaim> for AdoptGitSourceClaimDto {
    fn from(value: crate::core::adopt::AdoptGitSourceClaim) -> Self {
        Self {
            lock_path: value.lock_path.to_string_lossy().into_owned(),
            entry_name: value.entry_name,
            requested_ref: value.requested_ref,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptGitSourceHintDto {
    pub source_type: String,
    pub source_url: String,
    pub tracking_refs: Vec<String>,
    pub external_ownership_claims: Vec<AdoptGitSourceClaimDto>,
}

impl From<crate::core::adopt::AdoptGitSourceHint> for AdoptGitSourceHintDto {
    fn from(value: crate::core::adopt::AdoptGitSourceHint) -> Self {
        Self {
            source_type: value.source_type,
            source_url: value.source_url,
            tracking_refs: value.tracking_refs,
            external_ownership_claims: value
                .external_ownership_claims
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptLockEntryFaultDto {
    pub name: String,
    pub reason: String,
}

/// One discovered lock file in the ledger (spec §8.1).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptLockFileDto {
    pub path: String,
    pub fingerprint: String,
    pub byte_len: u64,
    pub version: u64,
    pub fault: Option<LockFileFaultDto>,
    pub entry_names: Vec<String>,
    pub entry_faults: Vec<AdoptLockEntryFaultDto>,
}

impl From<crate::seams::installer_lock_store::LockFileReport> for AdoptLockFileDto {
    fn from(value: crate::seams::installer_lock_store::LockFileReport) -> Self {
        Self {
            path: value.path.to_string_lossy().into_owned(),
            fingerprint: value.fingerprint,
            byte_len: value.byte_len,
            version: value.version,
            fault: value.fault.map(Into::into),
            entry_names: value.entries.into_iter().map(|entry| entry.name).collect(),
            entry_faults: value
                .entry_faults
                .into_iter()
                .map(|fault| AdoptLockEntryFaultDto {
                    name: fault.name,
                    reason: fault.reason,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptEvidenceReportDto {
    pub generation: u64,
    pub candidates: Vec<AdoptEvidenceCandidateDto>,
    pub git_sources: Vec<AdoptGitSourceHintDto>,
    pub lock_files: Vec<AdoptLockFileDto>,
    pub truncated: bool,
}

impl From<crate::core::adopt::AdoptEvidenceReport> for AdoptEvidenceReportDto {
    fn from(value: crate::core::adopt::AdoptEvidenceReport) -> Self {
        Self {
            generation: value.generation,
            candidates: value.candidates.into_iter().map(Into::into).collect(),
            git_sources: value.git_sources.into_iter().map(Into::into).collect(),
            lock_files: value.lock_files.into_iter().map(Into::into).collect(),
            truncated: value.truncated,
        }
    }
}

/// The three explicit Modified branches (spec §8.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifiedBranchDto {
    KeepCurrent,
    DiscardToAnchor,
    ConvertToLocalLink,
}

impl From<crate::core::adopt::ModifiedBranch> for ModifiedBranchDto {
    fn from(value: crate::core::adopt::ModifiedBranch) -> Self {
        use crate::core::adopt::ModifiedBranch as Branch;
        match value {
            Branch::KeepCurrent => Self::KeepCurrent,
            Branch::DiscardToAnchor => Self::DiscardToAnchor,
            Branch::ConvertToLocalLink => Self::ConvertToLocalLink,
        }
    }
}

impl From<ModifiedBranchDto> for crate::core::adopt::ModifiedBranch {
    fn from(value: ModifiedBranchDto) -> Self {
        match value {
            ModifiedBranchDto::KeepCurrent => Self::KeepCurrent,
            ModifiedBranchDto::DiscardToAnchor => Self::DiscardToAnchor,
            ModifiedBranchDto::ConvertToLocalLink => Self::ConvertToLocalLink,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptSelectionDto {
    pub canonical_entity: String,
    pub agent_ids: Vec<String>,
    pub modified_branch: Option<ModifiedBranchDto>,
    /// The user-chosen stable directory for move intents (spec §8.2,
    /// ADR-0013 §3); `None` for Home installs and stable Links.
    pub target_directory: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanAdoptRequestDto {
    pub evidence_generation: u64,
    pub selections: Vec<AdoptSelectionDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptTargetAgentDto {
    pub agent_id: String,
    pub name: String,
}

/// The frozen plan intent (spec §8.2): this slice only freezes evidence and
/// the handoff intent; lock and Home ownership changes are the handoff
/// ticket's work.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptPlanIntentDto {
    LocalLink,
    LocalLinkWithMove,
    RemoteInstallKeepCurrent,
    RemoteInstallDiscardModified,
    RemoteInstallConvertToLink,
}

impl From<crate::core::adopt::AdoptPlanIntent> for AdoptPlanIntentDto {
    fn from(value: crate::core::adopt::AdoptPlanIntent) -> Self {
        use crate::core::adopt::AdoptPlanIntent as Intent;
        match value {
            Intent::LocalLink => Self::LocalLink,
            Intent::LocalLinkWithMove => Self::LocalLinkWithMove,
            Intent::RemoteInstallKeepCurrent => Self::RemoteInstallKeepCurrent,
            Intent::RemoteInstallDiscardModified => Self::RemoteInstallDiscardModified,
            Intent::RemoteInstallConvertToLink => Self::RemoteInstallConvertToLink,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptPlanItemDto {
    pub directory_name: String,
    pub canonical_entity: String,
    pub intent: AdoptPlanIntentDto,
    pub final_entity_path: String,
    pub appearances: Vec<AdoptAppearanceEvidenceDto>,
    pub target_agents: Vec<AdoptTargetAgentDto>,
    pub applyable: bool,
    pub error: Option<String>,
}

impl From<crate::core::adopt::AdoptPlanItem> for AdoptPlanItemDto {
    fn from(value: crate::core::adopt::AdoptPlanItem) -> Self {
        Self {
            directory_name: value.directory_name,
            canonical_entity: value.canonical_entity.to_string_lossy().into_owned(),
            intent: value.intent.into(),
            final_entity_path: value.final_entity_path.to_string_lossy().into_owned(),
            appearances: value.appearances.into_iter().map(Into::into).collect(),
            target_agents: value
                .target_agents
                .into_iter()
                .map(|agent| AdoptTargetAgentDto {
                    agent_id: agent.agent_id.0,
                    name: agent.name,
                })
                .collect(),
            applyable: value.applyable,
            error: value.error,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptPlanDto {
    pub plan_token: String,
    pub evidence_generation: u64,
    pub items: Vec<AdoptPlanItemDto>,
    pub can_apply: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyAdoptRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptSkillResultDto {
    pub skill_id: String,
    pub directory_name: String,
    pub adopted: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptResultDto {
    pub operation_id: String,
    pub items: Vec<AdoptSkillResultDto>,
    pub snapshot_version: u64,
    pub undo_available: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoAdoptRequestDto {
    pub operation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptUndoItemResultDto {
    pub directory_name: String,
    pub undone: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptUndoResultDto {
    pub operation_id: String,
    pub items: Vec<AdoptUndoItemResultDto>,
    pub snapshot_version: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeAdoptRequestDto {
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelAdoptRequestDto {
    pub plan_token: String,
}

// -- Preferences & startup --

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPreferencesDto {
    pub launch_at_login: bool,
    pub show_in_dock: bool,
    pub check_app_updates: bool,
    pub check_skill_updates: bool,
}

impl From<AppPreferences> for AppPreferencesDto {
    fn from(value: AppPreferences) -> Self {
        Self {
            launch_at_login: value.launch_at_login,
            show_in_dock: value.show_in_dock,
            check_app_updates: value.check_app_updates,
            check_skill_updates: value.check_skill_updates,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceUpdatesDto {
    pub launch_at_login: Option<bool>,
    pub show_in_dock: Option<bool>,
    pub check_app_updates: Option<bool>,
    pub check_skill_updates: Option<bool>,
}

impl From<PreferenceUpdatesDto> for PreferenceUpdates {
    fn from(value: PreferenceUpdatesDto) -> Self {
        Self {
            launch_at_login: value.launch_at_login,
            show_in_dock: value.show_in_dock,
            check_app_updates: value.check_app_updates,
            check_skill_updates: value.check_skill_updates,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePreferencesResultDto {
    pub preferences: AppPreferencesDto,
    /// Closed runtime side-effect warnings (e.g. login item unavailable in a
    /// non-bundled development build); the persisted value is authoritative.
    /// `detail` is the raw technical fact, never App Copy.
    pub warning: Option<PreferencesWarningDto>,
}

/// Closed Preferences side-effect warnings (spec §4.7).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreferencesWarningDto {
    ShowInDockFailed { detail: String },
    LaunchAtLoginFailed { detail: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupAgentDto {
    pub id: String,
    pub name: String,
    pub kind: AgentKindDto,
    pub skills_path: String,
    pub detected: bool,
}

impl From<StartupAgent> for StartupAgentDto {
    fn from(value: StartupAgent) -> Self {
        Self {
            id: value.agent_id.0,
            name: value.name,
            kind: value.kind.into(),
            skills_path: value.skills_path.to_string_lossy().into_owned(),
            detected: value.detected,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentDirectoryRequestDto {
    pub agent_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupInfoDto {
    pub first_run: bool,
    pub library_path: Option<String>,
    pub agents: Vec<StartupAgentDto>,
}

/// Catalog access of a Bound Home (spec §4.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogAccessDto {
    ReadWrite,
    ReadOnly,
}

/// Why a Bound Home's Catalog is read-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogReadOnlyReasonDto {
    UnsupportedSchema,
    IntegrityFailed,
    OpenFailed,
}

/// Raw technical facts, never App Copy (spec §4.7): presentation-side copy
/// keys off `code`; `message` is the explicitly labeled raw detail.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticDto {
    pub code: String,
    pub message: String,
}

/// The closed top-level bootstrap route union (spec §4.2). `state` is the
/// discriminant; each variant carries only typed fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum BootstrapSnapshotDto {
    AppStateUnavailable {
        diagnostic: Option<DiagnosticDto>,
    },
    Unconfigured,
    DefaultHomeRecoveryOffer {
        path: String,
    },
    DefaultHomeRecoveryBlocked {
        path: String,
        reason: DefaultHomeRecoveryBlockedReasonDto,
    },
    Abandoned {
        home_id: String,
        path: String,
    },
    LegacyDetected {
        path: String,
    },
    FixtureRecoveryLocked {
        home_id: Option<String>,
        path: Option<String>,
    },
    HomeCandidatePending {
        path: String,
        operation_id: String,
    },
    Bound {
        home_id: String,
        catalog_access: CatalogAccessDto,
        catalog_readonly_reason: Option<CatalogReadOnlyReasonDto>,
        snapshot_version: u64,
    },
    HomeUnavailable {
        home_id: String,
        path: String,
        diagnostic: Option<DiagnosticDto>,
    },
    HomeIdentityMismatch {
        home_id: String,
        path: String,
        diagnostic: Option<DiagnosticDto>,
    },
}

/// Closed diagnostic reason for a default-path Existing Home Recovery block.
/// It is intentionally a code-only contract: no Home content, plan tokens or
/// credentials cross the Tauri boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultHomeRecoveryBlockedReasonDto {
    NotDirectory,
    MarkerMissingOrInvalid,
    LayoutCapabilities,
    CatalogMissing,
    CatalogUnreadable,
    CatalogIdentityMissing,
    HomeIdentityMismatch,
    CreationTimeMismatch,
    CatalogIntegrity,
    CatalogForeignKeys,
    CatalogCapabilities,
    ActiveWriter,
    OperationRecoveryRequired,
    FixtureContamination,
    Unreadable,
    RecoveryIneligible,
}

/// `bootstrap://changed` payload: isomorphic to the query snapshot plus the
/// write-gate generation so React can reconcile without re-querying.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapChangedPayloadDto {
    pub snapshot: BootstrapSnapshotDto,
    pub generation: u64,
}

// -- Home Lifecycle (spec §5.5, ADR-0012 §6) --

/// The high-friction Abandon preview: typed facts the confirmation dialog
/// shows; the user must retype the `home_id` to apply.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbandonPreviewDto {
    pub home_id: String,
    pub path: String,
    pub bound_at: String,
    pub plan_token: String,
}

/// The typed Abandon confirmation (ADR-0012 §6): `home_id` must exactly
/// match the current binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyAbandonRequestDto {
    pub plan_token: String,
    pub home_id: String,
}

/// Why Restore applies to a Bound Home (closed reason for presentation).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreReasonDto {
    CatalogIntegrityFailed,
    FixtureContamination,
}

/// Why Restore does not apply right now (closed reason for presentation).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreNotApplicableReasonDto {
    NoBinding,
    LegacyUnbound,
    AppStateUnavailable,
    IdentityMismatch,
    HomeUnavailable,
    UnsupportedSchema,
    OpenFailed,
    ActiveOperation,
}

/// Restore eligibility probe result (spec §5.5): only a provable
/// same-identity content failure is `restore_required`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreEligibilityDto {
    RestoreRequired {
        #[serde(rename = "homeId")]
        home_id: String,
        path: String,
        reason: RestoreReasonDto,
    },
    NotRequired,
    NotApplicable {
        reason: RestoreNotApplicableReasonDto,
    },
}

/// The closed public error union (spec §4.7): every command failure carries
/// a stable `code` plus typed fields where presentation needs them. App Copy
/// never crosses this boundary — React maps each code to a message key and
/// the optional `diagnostic` keeps the raw technical detail.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "code",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PublicErrorDto {
    Validation,
    NotFound,
    AgentConfigurationNameInvalid,
    AgentConfigurationNameConflict {
        name: String,
    },
    AgentPresetNotFound {
        #[serde(rename = "presetKey")]
        preset_key: String,
    },
    AgentRootRequired,
    AgentActivationTargetRequired,
    AgentRootDuplicate {
        path: String,
    },
    AgentRootOverlap {
        path: String,
        #[serde(rename = "conflictingPath")]
        conflicting_path: String,
    },
    AgentRootHomeOverlap {
        path: String,
    },
    AgentRootInvalid {
        path: String,
    },
    AgentTargetNotWritable {
        path: String,
    },
    AgentTargetNotAllowed {
        path: String,
    },
    AgentProjectSkillsDirInvalid,
    AgentConfigurationNotFound {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    AgentTargetInUse {
        #[serde(rename = "skillIds")]
        skill_ids: Vec<String>,
    },
    Conflict {
        #[serde(rename = "directoryName")]
        directory_name: String,
    },
    PlanStale,
    PermissionDenied,
    StateUnavailable,
    CatalogUnavailable,
    RecoveryRequired,
    SourceUnavailable,
    TargetMismatch,
    DiskFull {
        #[serde(rename = "requiredBytes")]
        required_bytes: u64,
        #[serde(rename = "availableBytes")]
        available_bytes: u64,
    },
    Modified,
    StaleUpdate,
    UpdateCancelled,
    DownloadFailed,
    InstallFailed,
    BootstrapUnavailable,
    RecoveryNotLocked,
    RecoveryNotPure,
    RestoreNotApplicable,
    RecoveryNoActiveOperation,
    RecoveryOperationAlreadyActive,
    RecoveryWriterActive,
    RecoveryStepFailed,
    RecoveryStateAmbiguous,
    RecoverySnapshotInUse,
    RecoverySnapshotNotQualified,
    RecoveryStateStore,
    RecoveryFilesystem,
    RecoveryProbe,
    ReconnectNotAvailable,
    AbandonNotApplicable,
    AbandonConfirmationMismatch,
    AbandonCasConflict,
    CandidateInvalid {
        /// Closed `snake_case` candidate-validation reason (spec §5.3);
        /// presentation maps it to a message key, never to free text.
        reason: String,
    },
    BindingStepFailed {
        cursor: String,
    },
    BindingStateAmbiguous,
    BindingNotCancellable,
    BindingMigrationFailed,
    ExistingHomeRecoveryIneligible {
        reason: RecoveryEligibilityRejectionDto,
    },
    ExistingHomeRecoveryProfileRejected {
        reason: RecoveryProfileRejectionDto,
    },
    LocaleStoreUnavailable,
    ScanNotWritable,
    ScanRunNotFound,
    ScanReportStale {
        #[serde(rename = "currentGeneration")]
        current_generation: u64,
    },
    ScanReportNotFound,
    ObservationPageStale {
        #[serde(rename = "currentGeneration")]
        current_generation: u64,
    },
    ObservationPageNotFound,
    Internal,
}

/// Closed command failure (spec §4.7): a typed public error plus optional raw
/// diagnostic. Never carries free-form App Copy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandFailureDto {
    pub error: PublicErrorDto,
    pub diagnostic: Option<DiagnosticDto>,
}

// -- Locale Authority (spec §4.5, §4.7) --

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLocaleSelectionRequestDto {
    pub selection: crate::seams::locale_store::LocaleSelection,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleSnapshotDto {
    pub selection: crate::seams::locale_store::LocaleSelection,
    pub effective_locale: crate::seams::locale_store::EffectiveLocale,
    pub generation: u64,
    pub diagnostic: Option<DiagnosticDto>,
}

impl From<crate::core::locale::LocaleSnapshot> for LocaleSnapshotDto {
    fn from(value: crate::core::locale::LocaleSnapshot) -> Self {
        Self {
            selection: value.selection,
            effective_locale: value.effective_locale,
            generation: value.generation,
            diagnostic: value.diagnostic.map(|diagnostic| DiagnosticDto {
                code: diagnostic.code,
                message: diagnostic.message,
            }),
        }
    }
}

// -- Fixture Recovery (spec §4.4, §5.2) --

/// Which Home shape the recovery operates on.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RecoveryModeDto {
    LegacyUnbound,
    BoundRestore { home_id: String },
}

/// The closed fixture classification; `mixed`/`unknown` carry the stable
/// deviation codes the UI renders (never free-form App Copy).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FixtureClassificationDto {
    Pure,
    Mixed { reasons: Vec<String> },
    Unknown { reasons: Vec<String> },
    Clean,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEvidenceDto {
    pub tables: Vec<String>,
    pub schema_version: Option<u32>,
    pub first_run_completed_at: Option<String>,
    pub skill_row_count: u64,
    pub agent_row_count: u64,
    pub activation_row_count: u64,
    pub file_source_row_count: u64,
    pub remote_source_row_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeEvidenceDto {
    pub fixture_entities_present: bool,
    pub skill_authoring_hash_matches: Option<bool>,
    pub media_xray_hash_matches: Option<bool>,
    pub root_hash_matches: Option<bool>,
    pub legacy_audit_entity_present: bool,
}

/// The active recovery operation, when the lock is held by one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRecoveryOperationDto {
    pub operation_id: String,
    pub cursor: Option<String>,
    pub snapshot_path: Option<String>,
    pub prepared_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureRecoveryPreviewDto {
    pub mode: RecoveryModeDto,
    pub path: String,
    pub classification: FixtureClassificationDto,
    pub catalog_evidence: CatalogEvidenceDto,
    pub tree_evidence: TreeEvidenceDto,
    pub can_preview: bool,
    pub active_operation: Option<ActiveRecoveryOperationDto>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFixtureRecoveryRequestDto {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureRecoveryPlanDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyFixtureRecoveryRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryResultDto {
    pub operation_id: String,
    pub awaiting_commit: bool,
    pub rolled_back: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmFixtureRecoveryRequestDto {
    pub operation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetySnapshotDto {
    pub snapshot_id: String,
    pub path: String,
    pub manifest_hash: Option<String>,
    pub file_count: u64,
    pub total_bytes: u64,
    pub taken_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSafetySnapshotRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSnapshotPreviewDto {
    pub snapshot_id: String,
    pub path: String,
    pub file_count: u64,
    pub total_bytes: u64,
}

// -- Home Binding (spec §4.2, §5.3–§5.4) --

/// Which one-time transition a prepared candidate carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateModeDto {
    Fresh,
    LegacyInPlace,
    LegacyCopy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareHomeRequestDto {
    pub path: String,
}

// -- Existing Home Recovery (issues #66–#67) --

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareExistingHomeRecoveryRequestDto {
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelExistingHomeRecoveryRequestDto {
    pub plan_token: String,
}

/// Confirming Existing Home Recovery deliberately consumes only the opaque
/// plan token. The verified Home identity is a reviewed fact, never text the
/// user must retype.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmExistingHomeRecoveryRequestDto {
    pub plan_token: String,
}

/// Closed recovery eligibility reasons. This is deliberately separate from
/// Bootstrap's broader state union: #66 exposes only the four conditions
/// that can close a recovery preview.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryEligibilityRejectionDto {
    CurrentBinding,
    AbandonedHistory,
    ActiveRecoveryLedger,
    BootstrapState,
}

impl From<crate::core::existing_home_recovery::RecoveryEligibilityRejection>
    for RecoveryEligibilityRejectionDto
{
    fn from(value: crate::core::existing_home_recovery::RecoveryEligibilityRejection) -> Self {
        use crate::core::existing_home_recovery::RecoveryEligibilityRejection;

        match value {
            RecoveryEligibilityRejection::CurrentBinding => Self::CurrentBinding,
            RecoveryEligibilityRejection::AbandonedHistory => Self::AbandonedHistory,
            RecoveryEligibilityRejection::ActiveRecoveryLedger => Self::ActiveRecoveryLedger,
            RecoveryEligibilityRejection::BootstrapState => Self::BootstrapState,
        }
    }
}

impl RecoveryEligibilityRejectionDto {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::CurrentBinding => "current_binding",
            Self::AbandonedHistory => "abandoned_history",
            Self::ActiveRecoveryLedger => "active_recovery_ledger",
            Self::BootstrapState => "bootstrap_state",
        }
    }
}

/// Closed Recovery Profile rejections. The wire contract permits no raw path,
/// Catalog content or token facts in a user-visible failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryProfileRejectionDto {
    NotDirectory,
    MarkerMissingOrInvalid,
    LayoutCapabilities,
    CatalogMissing,
    CatalogUnreadable,
    CatalogIdentityMissing,
    HomeIdentityMismatch,
    CreationTimeMismatch,
    CatalogIntegrity,
    CatalogForeignKeys,
    CatalogCapabilities,
    ActiveWriter,
    OperationRecoveryRequired,
    FixtureContamination,
}

impl From<crate::core::existing_home_recovery::RecoveryProfileRejection>
    for RecoveryProfileRejectionDto
{
    fn from(value: crate::core::existing_home_recovery::RecoveryProfileRejection) -> Self {
        use crate::core::existing_home_recovery::RecoveryProfileRejection;

        match value {
            RecoveryProfileRejection::NotDirectory => Self::NotDirectory,
            RecoveryProfileRejection::MarkerMissingOrInvalid => Self::MarkerMissingOrInvalid,
            RecoveryProfileRejection::LayoutCapabilities => Self::LayoutCapabilities,
            RecoveryProfileRejection::CatalogMissing => Self::CatalogMissing,
            RecoveryProfileRejection::CatalogUnreadable => Self::CatalogUnreadable,
            RecoveryProfileRejection::CatalogIdentityMissing => Self::CatalogIdentityMissing,
            RecoveryProfileRejection::HomeIdentityMismatch => Self::HomeIdentityMismatch,
            RecoveryProfileRejection::CreationTimeMismatch => Self::CreationTimeMismatch,
            RecoveryProfileRejection::CatalogIntegrity => Self::CatalogIntegrity,
            RecoveryProfileRejection::CatalogForeignKeys => Self::CatalogForeignKeys,
            RecoveryProfileRejection::CatalogCapabilities => Self::CatalogCapabilities,
            RecoveryProfileRejection::ActiveWriter => Self::ActiveWriter,
            RecoveryProfileRejection::OperationRecoveryRequired => Self::OperationRecoveryRequired,
            RecoveryProfileRejection::FixtureContamination => Self::FixtureContamination,
        }
    }
}

impl RecoveryProfileRejectionDto {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::NotDirectory => "not_directory",
            Self::MarkerMissingOrInvalid => "marker_missing_or_invalid",
            Self::LayoutCapabilities => "layout_capabilities",
            Self::CatalogMissing => "catalog_missing",
            Self::CatalogUnreadable => "catalog_unreadable",
            Self::CatalogIdentityMissing => "catalog_identity_missing",
            Self::HomeIdentityMismatch => "home_identity_mismatch",
            Self::CreationTimeMismatch => "creation_time_mismatch",
            Self::CatalogIntegrity => "catalog_integrity",
            Self::CatalogForeignKeys => "catalog_foreign_keys",
            Self::CatalogCapabilities => "catalog_capabilities",
            Self::ActiveWriter => "active_writer",
            Self::OperationRecoveryRequired => "operation_recovery_required",
            Self::FixtureContamination => "fixture_contamination",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryProfileFactDto {
    MarkerCatalogIdentity,
    StandardLayout,
    CatalogIntegrity,
    CatalogForeignKeys,
    CatalogCapabilities,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingHomeRecoveryPlanDto {
    pub path: String,
    pub home_id: String,
    pub created_at: String,
    pub plan_token: String,
    pub facts: Vec<RecoveryProfileFactDto>,
}

impl From<crate::core::existing_home_recovery::ExistingHomeRecoveryPlan>
    for ExistingHomeRecoveryPlanDto
{
    fn from(value: crate::core::existing_home_recovery::ExistingHomeRecoveryPlan) -> Self {
        let facts = value.facts;
        let mut fact_dtos = Vec::new();
        if facts.marker_catalog_identity {
            fact_dtos.push(RecoveryProfileFactDto::MarkerCatalogIdentity);
        }
        if facts.standard_layout {
            fact_dtos.push(RecoveryProfileFactDto::StandardLayout);
        }
        if facts.catalog_integrity {
            fact_dtos.push(RecoveryProfileFactDto::CatalogIntegrity);
        }
        if facts.catalog_foreign_keys {
            fact_dtos.push(RecoveryProfileFactDto::CatalogForeignKeys);
        }
        if facts.catalog_capabilities {
            fact_dtos.push(RecoveryProfileFactDto::CatalogCapabilities);
        }
        Self {
            path: value.path.to_string_lossy().into_owned(),
            home_id: value.home_id.0,
            created_at: value.created_at,
            plan_token: value.plan_token,
            facts: fact_dtos,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmHomeRequestDto {
    pub candidate_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateOperationRequestDto {
    pub operation_id: String,
}

/// A validated, not-yet-confirmed Home candidate: the token `confirm_home`
/// binds to this path and mode, plus the typed facts React renders.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeCandidateDto {
    pub path: String,
    pub token: String,
    pub mode: CandidateModeDto,
    pub volume_fsid: String,
    pub volume_uuid: String,
    pub available_bytes: u64,
    pub legacy_source: Option<String>,
}

impl From<StartupInfo> for StartupInfoDto {
    fn from(value: StartupInfo) -> Self {
        Self {
            first_run: value.first_run,
            library_path: value
                .library_path
                .map(|path| path.to_string_lossy().into_owned()),
            agents: value
                .agents
                .into_iter()
                .map(StartupAgentDto::from)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;

    #[test]
    fn source_group_preview_outcomes_keep_the_typed_wire_contract() {
        let claim = ExternalOwnershipClaim {
            lock_path: PathBuf::from("/locks/source.lock.json"),
            entry_name: "legacy-skill".into(),
            requested_ref: "main".into(),
        };
        let preview = SourceGroupPreviewOutcomeDto::from(SourceGroupPreviewOutcome::Preview(
            SourceGroupPreview {
                provider: "github".into(),
                source_url: "https://github.com/acme/source".into(),
                tracking_ref: "main".into(),
                resolved_commit: "a".repeat(40),
                members: vec![SourceGroupMember {
                    directory_name: "skill-a".into(),
                    display_name: "Skill A".into(),
                    description: "A complete member".into(),
                    skill_path: "skills/skill-a".into(),
                    tree_summary: "3 files".into(),
                }],
                external_ownership_claims: vec![claim.clone()],
            },
        ));
        let conflict = SourceGroupPreviewOutcomeDto::from(
            SourceGroupPreviewOutcome::RepositoryRefConflict(RepositoryRefConflict {
                provider: "gitlab".into(),
                source_url: "https://gitlab.com/acme/source".into(),
                available_refs: vec!["main".into(), "release".into()],
                external_ownership_claims: vec![claim.clone()],
            }),
        );
        let split = SourceGroupPreviewOutcomeDto::from(
            SourceGroupPreviewOutcome::RepositoryOwnershipSplit(RepositoryOwnershipSplit {
                provider: "git".into(),
                source_url: "https://example.com/acme/source".into(),
                tracking_ref: "main".into(),
                lock_paths: vec![
                    PathBuf::from("/locks/one.lock.json"),
                    PathBuf::from("/locks/two.lock.json"),
                ],
                external_ownership_claims: vec![claim],
            }),
        );

        assert_eq!(
            serde_json::to_value(preview).expect("serialize preview"),
            json!({
                "kind": "preview",
                "preview": {
                    "provider": "github",
                    "sourceUrl": "https://github.com/acme/source",
                    "trackingRef": "main",
                    "resolvedCommit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "members": [{
                        "directoryName": "skill-a",
                        "displayName": "Skill A",
                        "description": "A complete member",
                        "skillPath": "skills/skill-a",
                        "treeSummary": "3 files"
                    }],
                    "externalOwnershipClaims": [{
                        "lockPath": "/locks/source.lock.json",
                        "entryName": "legacy-skill",
                        "requestedRef": "main"
                    }]
                }
            })
        );
        assert_eq!(
            serde_json::to_value(conflict).expect("serialize conflict"),
            json!({
                "kind": "repository_ref_conflict",
                "conflict": {
                    "provider": "gitlab",
                    "sourceUrl": "https://gitlab.com/acme/source",
                    "availableRefs": ["main", "release"],
                    "externalOwnershipClaims": [{
                        "lockPath": "/locks/source.lock.json",
                        "entryName": "legacy-skill",
                        "requestedRef": "main"
                    }]
                }
            })
        );
        assert_eq!(
            serde_json::to_value(split).expect("serialize split"),
            json!({
                "kind": "repository_ownership_split",
                "split": {
                    "provider": "git",
                    "sourceUrl": "https://example.com/acme/source",
                    "trackingRef": "main",
                    "lockPaths": ["/locks/one.lock.json", "/locks/two.lock.json"],
                    "externalOwnershipClaims": [{
                        "lockPath": "/locks/source.lock.json",
                        "entryName": "legacy-skill",
                        "requestedRef": "main"
                    }]
                }
            })
        );
    }
}
