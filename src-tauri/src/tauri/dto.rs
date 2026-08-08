use serde::{Deserialize, Serialize};

use crate::core::activation::{ActivationPlanKind, ActivationPreview, ActivationResult};
use crate::core::domain::{
    ActivationObservedState, AgentActivation, AgentKind, CatalogFilter, Compatibility, Health,
    SkillDetail, SkillSummary, SourceKind,
};
use crate::core::import::{
    FileImportCandidate, FileImportDiscovery, FileImportPreview, FileImportResult,
    FileImportSelectionPreview, FileImportSelectionResult, GitImportCandidate, GitImportDiscovery,
    GitImportPreview, GitImportResult, GitImportSelectionPreview, GitImportSelectionResult,
    LibraryConflict, LinkImportCandidate, LinkImportPreview, LinkImportResult,
};
use crate::core::maintenance::ActivationHealthReport;
use crate::core::update::{
    UpdateCheckGroup, UpdateCheckItem, UpdateCheckReport, UpdateItemResult, UpdatePlan,
    UpdatePlanItem, UpdateResult,
};

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
    pub source_label: String,
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
            source_label: value.source_label,
            frontmatter_name: value.frontmatter_name,
            last_activity_at: value.last_activity_at,
            skill_markdown: value.skill_markdown,
        }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationObservedStateDto {
    Present,
    Missing,
    TargetMismatch,
    Dangling,
    Occupied,
}

impl From<ActivationObservedState> for ActivationObservedStateDto {
    fn from(value: ActivationObservedState) -> Self {
        match value {
            ActivationObservedState::Present => Self::Present,
            ActivationObservedState::Missing => Self::Missing,
            ActivationObservedState::TargetMismatch => Self::TargetMismatch,
            ActivationObservedState::Dangling => Self::Dangling,
            ActivationObservedState::Occupied => Self::Occupied,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivationDto {
    pub id: String,
    pub name: String,
    pub kind: AgentKindDto,
    pub skills_path: String,
    pub detected: bool,
    pub compatibility: CompatibilityDto,
    pub desired_enabled: bool,
    pub observed_state: ActivationObservedStateDto,
}

impl From<AgentActivation> for AgentActivationDto {
    fn from(value: AgentActivation) -> Self {
        Self {
            id: value.id.0,
            name: value.name,
            kind: value.kind.into(),
            skills_path: value.skills_path,
            detected: value.detected,
            compatibility: value.compatibility.into(),
            desired_enabled: value.desired_enabled,
            observed_state: value.observed_state.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandErrorDto {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanActivationRequestDto {
    pub skill_id: String,
    pub agent_id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanActivationRepairRequestDto {
    pub skill_id: String,
    pub agent_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyActivationRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelActivationRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationPreviewDto {
    pub plan_token: String,
    pub skill_directory_name: String,
    pub agent_name: String,
    pub enabled: bool,
    pub kind: ActivationPlanKindDto,
    pub entry_path: String,
    pub target_path: String,
    pub compatibility_warning: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationPlanKindDto {
    Enable,
    Disable,
    Repair,
}

impl From<ActivationPlanKind> for ActivationPlanKindDto {
    fn from(value: ActivationPlanKind) -> Self {
        match value {
            ActivationPlanKind::Enable => Self::Enable,
            ActivationPlanKind::Disable => Self::Disable,
            ActivationPlanKind::Repair => Self::Repair,
        }
    }
}

impl From<ActivationPreview> for ActivationPreviewDto {
    fn from(value: ActivationPreview) -> Self {
        Self {
            plan_token: value.plan_token,
            skill_directory_name: value.skill_directory_name,
            agent_name: value.agent_name,
            enabled: value.enabled,
            kind: value.kind.into(),
            entry_path: value.entry_path.to_string_lossy().into_owned(),
            target_path: value.target_path.to_string_lossy().into_owned(),
            compatibility_warning: value.compatibility_warning,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationResultDto {
    pub skill_id: String,
    pub agent_id: String,
    pub desired_enabled: bool,
    pub observed_state: ActivationObservedStateDto,
    pub snapshot_version: u64,
}

impl From<ActivationResult> for ActivationResultDto {
    fn from(value: ActivationResult) -> Self {
        Self {
            skill_id: value.skill_id.0,
            agent_id: value.agent_id.0,
            desired_enabled: value.desired_enabled,
            observed_state: value.observed_state.into(),
            snapshot_version: value.snapshot_version,
        }
    }
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

/// Spec §5.3: persisted times may be integer epoch, DTOs always output RFC 3339.
fn epoch_seconds_to_rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60,
    )
}

/// Howard Hinnant's civil-from-days algorithm (proleptic Gregorian calendar).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

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
    pub errors: Vec<String>,
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
