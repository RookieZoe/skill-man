use serde::{Deserialize, Serialize};

use crate::core::activation::{ActivationPreview, ActivationResult};
use crate::core::domain::{
    ActivationObservedState, AgentActivation, AgentKind, CatalogFilter, Compatibility, Health,
    SkillDetail, SkillSummary, SourceKind,
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
    pub entry_path: String,
    pub target_path: String,
}

impl From<ActivationPreview> for ActivationPreviewDto {
    fn from(value: ActivationPreview) -> Self {
        Self {
            plan_token: value.plan_token,
            skill_directory_name: value.skill_directory_name,
            agent_name: value.agent_name,
            enabled: value.enabled,
            entry_path: value.entry_path.to_string_lossy().into_owned(),
            target_path: value.target_path.to_string_lossy().into_owned(),
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
