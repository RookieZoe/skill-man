use serde::Deserialize;

use crate::core::domain::{
    ActivationObservedState, AgentActivation, AgentId, AgentKind, CatalogFilter, Compatibility,
    Health, SkillDetail, SkillId, SkillSummary, SourceKind,
};
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};

const LIBRARY_DESK_FIXTURE: &str = include_str!("../../../fixtures/library-desk.json");

pub struct FixtureCatalogStore {
    snapshot_version: u64,
    skills: Vec<SkillDetail>,
    agents: Vec<FixtureAgent>,
}

impl FixtureCatalogStore {
    pub fn library_desk() -> Self {
        let fixture: FixtureFile =
            serde_json::from_str(LIBRARY_DESK_FIXTURE).expect("valid Library Desk fixture");
        let agents = fixture.agents;
        let mut skills: Vec<SkillDetail> =
            fixture.skills.into_iter().map(SkillDetail::from).collect();
        for skill in &mut skills {
            skill.summary.enabled_agent_count = agents
                .iter()
                .filter(|agent| agent.enabled_skill_ids.contains(&skill.summary.id.0))
                .count() as u32;
        }
        Self {
            snapshot_version: fixture.snapshot_version,
            skills,
            agents,
        }
    }
}

impl CatalogStore for FixtureCatalogStore {
    fn snapshot_version(&self) -> u64 {
        self.snapshot_version
    }

    fn list(&self, filter: CatalogFilter) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        Ok(self
            .skills
            .iter()
            .map(|detail| &detail.summary)
            .filter(|skill| filter.includes(skill))
            .cloned()
            .collect())
    }

    fn inspect(&self, skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError> {
        Ok(self
            .skills
            .iter()
            .find(|detail| detail.summary.id == *skill_id)
            .cloned())
    }

    fn list_agents(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<Vec<AgentActivation>>, CatalogStoreError> {
        if !self
            .skills
            .iter()
            .any(|detail| detail.summary.id == *skill_id)
        {
            return Ok(None);
        }

        Ok(Some(
            self.agents
                .iter()
                .map(|agent| agent.activation_for(skill_id))
                .collect(),
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureFile {
    snapshot_version: u64,
    skills: Vec<FixtureSkill>,
    agents: Vec<FixtureAgent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureSkill {
    id: String,
    directory_name: String,
    display_name: String,
    description: String,
    source_kind: FixtureSourceKind,
    health: FixtureHealth,
    final_entity_path: String,
    source_label: String,
    frontmatter_name: Option<String>,
    last_activity_at: String,
    skill_markdown: String,
}

impl From<FixtureSkill> for SkillDetail {
    fn from(value: FixtureSkill) -> Self {
        Self {
            summary: SkillSummary {
                id: SkillId(value.id),
                directory_name: value.directory_name,
                display_name: value.display_name,
                description: value.description,
                source_kind: value.source_kind.into(),
                health: value.health.into(),
                enabled_agent_count: 0,
            },
            final_entity_path: value.final_entity_path,
            source_label: value.source_label,
            frontmatter_name: value.frontmatter_name,
            last_activity_at: value.last_activity_at,
            skill_markdown: value.skill_markdown,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureSourceKind {
    Link,
    RemoteInstall,
    FileInstall,
}

impl From<FixtureSourceKind> for SourceKind {
    fn from(value: FixtureSourceKind) -> Self {
        match value {
            FixtureSourceKind::Link => Self::Link,
            FixtureSourceKind::RemoteInstall => Self::RemoteInstall,
            FixtureSourceKind::FileInstall => Self::FileInstall,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureHealth {
    Healthy,
    Broken,
    Modified,
}

impl From<FixtureHealth> for Health {
    fn from(value: FixtureHealth) -> Self {
        match value {
            FixtureHealth::Healthy => Self::Healthy,
            FixtureHealth::Broken => Self::Broken,
            FixtureHealth::Modified => Self::Modified,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureAgent {
    id: String,
    name: String,
    kind: FixtureAgentKind,
    skills_path: String,
    detected: bool,
    compatibility: FixtureCompatibility,
    enabled_skill_ids: Vec<String>,
}

impl FixtureAgent {
    fn activation_for(&self, skill_id: &SkillId) -> AgentActivation {
        let desired_enabled = self.enabled_skill_ids.contains(&skill_id.0);
        AgentActivation {
            id: AgentId(self.id.clone()),
            name: self.name.clone(),
            kind: self.kind.into(),
            skills_path: self.skills_path.clone(),
            detected: self.detected,
            compatibility: self.compatibility.into(),
            desired_enabled,
            observed_state: if desired_enabled {
                ActivationObservedState::Present
            } else {
                ActivationObservedState::Missing
            },
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureAgentKind {
    ClaudePreset,
    CodexPreset,
    Custom,
}

impl From<FixtureAgentKind> for AgentKind {
    fn from(value: FixtureAgentKind) -> Self {
        match value {
            FixtureAgentKind::ClaudePreset => Self::ClaudePreset,
            FixtureAgentKind::CodexPreset => Self::CodexPreset,
            FixtureAgentKind::Custom => Self::Custom,
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureCompatibility {
    Verified,
    Unknown,
}

impl From<FixtureCompatibility> for Compatibility {
    fn from(value: FixtureCompatibility) -> Self {
        match value {
            FixtureCompatibility::Verified => Self::Verified,
            FixtureCompatibility::Unknown => Self::Unknown,
        }
    }
}
