use serde::{Deserialize, Serialize};

use crate::core::activation::{
    ActivationConflictDetails, ActivationPlanKind, ActivationPreview, ActivationReplacePreview,
    ActivationReplaceUndoResult, ActivationResult, OccupierKind, OccupierSummary,
};
use crate::core::adopt::{AdoptAppearance, AdoptAppearanceKind, AdoptCandidate, AdoptRisk};
use crate::core::app_update::{
    AppUpdateCheck, AppUpdateOffer, CancelledAppUpdate, DownloadedAppUpdate,
};
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
use crate::core::maintenance::{
    ActivationHealthReport, RelocatePreview, RelocateResult, RemovePreview, RemoveResult,
};
use crate::core::startup::{StartupAgent, StartupInfo};
use crate::core::update::{
    UpdateCheckGroup, UpdateCheckItem, UpdateCheckReport, UpdateItemResult, UpdatePlan,
    UpdatePlanItem, UpdateResult,
};
use crate::seams::preferences_store::{AppPreferences, PreferenceUpdates};

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
    pub skill_id: String,
    pub agent_id: String,
    pub skill_directory_name: String,
    pub agent_name: String,
    pub enabled: bool,
    pub kind: ActivationPlanKindDto,
    pub entry_path: String,
    pub target_path: String,
    pub compatibility_warning: Option<CompatibilityWarningDto>,
}

/// Closed compatibility warnings (spec §4.7): presentation composes copy
/// from the typed variant, never from a free string.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompatibilityWarningDto {
    CustomUnknown,
    FrontmatterMismatch {
        #[serde(rename = "frontmatterName")]
        frontmatter_name: String,
        #[serde(rename = "directoryName")]
        directory_name: String,
    },
}

impl From<crate::seams::agent_adapter::CompatibilityWarning> for CompatibilityWarningDto {
    fn from(value: crate::seams::agent_adapter::CompatibilityWarning) -> Self {
        match value {
            crate::seams::agent_adapter::CompatibilityWarning::CustomUnknown => Self::CustomUnknown,
            crate::seams::agent_adapter::CompatibilityWarning::FrontmatterMismatch {
                frontmatter_name,
                directory_name,
            } => Self::FrontmatterMismatch {
                frontmatter_name,
                directory_name,
            },
        }
    }
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
            skill_id: value.skill_id.0,
            agent_id: value.agent_id.0,
            skill_directory_name: value.skill_directory_name,
            agent_name: value.agent_name,
            enabled: value.enabled,
            kind: value.kind.into(),
            entry_path: value.entry_path.to_string_lossy().into_owned(),
            target_path: value.target_path.to_string_lossy().into_owned(),
            compatibility_warning: value.compatibility_warning.map(Into::into),
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

// -- Activation Conflict (Enable 遇占用三选一) --

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationConflictRequestDto {
    pub skill_id: String,
    pub agent_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OccupierKindDto {
    RealDirectory,
    Symlink,
    File,
}

impl From<OccupierKind> for OccupierKindDto {
    fn from(value: OccupierKind) -> Self {
        match value {
            OccupierKind::RealDirectory => Self::RealDirectory,
            OccupierKind::Symlink => Self::Symlink,
            OccupierKind::File => Self::File,
        }
    }
}

/// Closed reasons why an occupier cannot be Adopted (spec §4.7).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OccupierNotAdoptableReasonDto {
    RegularFile,
    PointsAtManagedSkill,
    PointsAtThisSkill,
    NoReadableSkillMd,
    TargetUnresolvable,
    IdentityConflict {
        #[serde(rename = "directoryName")]
        directory_name: String,
    },
}

impl From<crate::core::activation::OccupierNotAdoptableReason> for OccupierNotAdoptableReasonDto {
    fn from(value: crate::core::activation::OccupierNotAdoptableReason) -> Self {
        use crate::core::activation::OccupierNotAdoptableReason as Reason;
        match value {
            Reason::RegularFile => Self::RegularFile,
            Reason::PointsAtManagedSkill => Self::PointsAtManagedSkill,
            Reason::PointsAtThisSkill => Self::PointsAtThisSkill,
            Reason::NoReadableSkillMd => Self::NoReadableSkillMd,
            Reason::TargetUnresolvable => Self::TargetUnresolvable,
            Reason::IdentityConflict { directory_name } => {
                Self::IdentityConflict { directory_name }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OccupierSummaryDto {
    pub kind: OccupierKindDto,
    pub symlink_target: Option<String>,
    pub final_entity_path: Option<String>,
    pub directory_name: String,
    pub is_skill: bool,
    pub adoptable: bool,
    pub not_adoptable_reason: Option<OccupierNotAdoptableReasonDto>,
}

impl From<OccupierSummary> for OccupierSummaryDto {
    fn from(value: OccupierSummary) -> Self {
        Self {
            kind: value.kind.into(),
            symlink_target: value
                .symlink_target
                .map(|path| path.to_string_lossy().into_owned()),
            final_entity_path: value
                .final_entity_path
                .map(|path| path.to_string_lossy().into_owned()),
            directory_name: value.directory_name,
            is_skill: value.is_skill,
            adoptable: value.adoptable,
            not_adoptable_reason: value.not_adoptable_reason.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationConflictDetailsDto {
    pub skill_id: String,
    pub agent_id: String,
    pub entry_path: String,
    pub target_path: String,
    pub occupier: OccupierSummaryDto,
}

impl From<ActivationConflictDetails> for ActivationConflictDetailsDto {
    fn from(value: ActivationConflictDetails) -> Self {
        Self {
            skill_id: value.skill_id.0,
            agent_id: value.agent_id.0,
            entry_path: value.entry_path.to_string_lossy().into_owned(),
            target_path: value.target_path.to_string_lossy().into_owned(),
            occupier: value.occupier.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanActivationReplaceRequestDto {
    pub skill_id: String,
    pub agent_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationReplacePreviewDto {
    pub plan_token: String,
    pub operation_id: String,
    pub skill_directory_name: String,
    pub agent_name: String,
    pub entry_path: String,
    pub target_path: String,
    pub backup_path: String,
    pub occupant_kind: OccupierKindDto,
}

impl From<ActivationReplacePreview> for ActivationReplacePreviewDto {
    fn from(value: ActivationReplacePreview) -> Self {
        Self {
            plan_token: value.plan_token,
            operation_id: value.operation_id,
            skill_directory_name: value.skill_directory_name,
            agent_name: value.agent_name,
            entry_path: value.entry_path.to_string_lossy().into_owned(),
            target_path: value.target_path.to_string_lossy().into_owned(),
            backup_path: value.backup_path.to_string_lossy().into_owned(),
            occupant_kind: value.occupant_kind.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyActivationReplaceRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelActivationReplaceRequestDto {
    pub plan_token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoActivationReplaceRequestDto {
    pub operation_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeActivationReplaceRequestDto {
    pub operation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationReplaceUndoResultDto {
    pub undone: bool,
    pub error: Option<String>,
    pub snapshot_version: u64,
}

impl From<ActivationReplaceUndoResult> for ActivationReplaceUndoResultDto {
    fn from(value: ActivationReplaceUndoResult) -> Self {
        Self {
            undone: value.undone,
            error: value.error,
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

// -- Adopt --

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptRiskDto {
    None,
    External,
    Broken,
}

impl From<AdoptRisk> for AdoptRiskDto {
    fn from(value: AdoptRisk) -> Self {
        match value {
            AdoptRisk::None => Self::None,
            AdoptRisk::External => Self::External,
            AdoptRisk::Broken => Self::Broken,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptAppearanceDto {
    pub entry_path: String,
    pub kind: String,
    pub agent_id: Option<String>,
    pub shared: bool,
}

impl From<AdoptAppearance> for AdoptAppearanceDto {
    fn from(value: AdoptAppearance) -> Self {
        Self {
            entry_path: value.entry_path.to_string_lossy().into_owned(),
            kind: match value.kind {
                AdoptAppearanceKind::RealDirectory => "real_directory".into(),
                AdoptAppearanceKind::Symlink { .. } => "symlink".into(),
            },
            agent_id: value.agent_id.map(|agent_id| agent_id.0),
            shared: value.shared,
        }
    }
}

/// Closed reasons for a non-safe Adopt candidate (spec §4.7); `UnsafeTree`
/// carries the raw filesystem validation fact, never App Copy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdoptRiskReasonDto {
    Dangling,
    OutsideHome { path: String },
    InstallerManaged { path: String },
    UnsafeTree { detail: String },
}

impl From<crate::core::adopt::AdoptRiskReason> for AdoptRiskReasonDto {
    fn from(value: crate::core::adopt::AdoptRiskReason) -> Self {
        use crate::core::adopt::AdoptRiskReason as Reason;
        match value {
            Reason::Dangling => Self::Dangling,
            Reason::OutsideHome { path } => Self::OutsideHome {
                path: path.to_string_lossy().into_owned(),
            },
            Reason::InstallerManaged { path } => Self::InstallerManaged {
                path: path.to_string_lossy().into_owned(),
            },
            Reason::UnsafeTree(detail) => Self::UnsafeTree { detail },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptCandidateDto {
    pub canonical_entity: String,
    pub directory_name: String,
    pub directory_names: Vec<String>,
    pub appearances: Vec<AdoptAppearanceDto>,
    pub risk: AdoptRiskDto,
    pub risk_reason: Option<AdoptRiskReasonDto>,
    pub conflict: Option<LibraryConflictDto>,
    pub adoptable: bool,
    pub suggested_agent_ids: Vec<String>,
}

impl From<AdoptCandidate> for AdoptCandidateDto {
    fn from(value: AdoptCandidate) -> Self {
        Self {
            canonical_entity: value.canonical_entity.to_string_lossy().into_owned(),
            directory_name: value.directory_name,
            directory_names: value.directory_names,
            appearances: value
                .appearances
                .into_iter()
                .map(AdoptAppearanceDto::from)
                .collect(),
            risk: value.risk.into(),
            risk_reason: value.risk_reason.map(Into::into),
            conflict: value.conflict.map(LibraryConflictDto::from),
            adoptable: value.adoptable,
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
pub struct AdoptScanReportDto {
    pub candidates: Vec<AdoptCandidateDto>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptSelectionDto {
    pub canonical_entity: String,
    pub agent_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanAdoptRequestDto {
    pub selections: Vec<AdoptSelectionDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptTargetAgentDto {
    pub agent_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptPlanItemDto {
    pub directory_name: String,
    pub canonical_entity: String,
    pub kind: String,
    pub final_entity_path: String,
    pub appearances: Vec<AdoptAppearanceDto>,
    pub target_agents: Vec<AdoptTargetAgentDto>,
    pub adoptable: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptPlanDto {
    pub plan_token: String,
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

/// `bootstrap://changed` payload: isomorphic to the query snapshot plus the
/// write-gate generation so React can reconcile without re-querying.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapChangedPayloadDto {
    pub snapshot: BootstrapSnapshotDto,
    pub generation: u64,
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
    RecoveryNoActiveOperation,
    RecoveryOperationAlreadyActive,
    RecoveryWriterActive,
    RecoveryStepFailed,
    RecoveryStateAmbiguous,
    RecoverySnapshotInUse,
    RecoveryStateStore,
    RecoveryFilesystem,
    RecoveryProbe,
    LocaleStoreUnavailable,
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

impl From<StartupInfo> for StartupInfoDto {
    fn from(value: StartupInfo) -> Self {
        Self {
            first_run: value.first_run,
            agents: value
                .agents
                .into_iter()
                .map(StartupAgentDto::from)
                .collect(),
        }
    }
}
