use std::sync::RwLock;

use serde::Deserialize;

use crate::core::domain::{CatalogFilter, Health, SkillDetail, SkillId, SkillSummary, SourceKind};
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};

const LIBRARY_DESK_FIXTURE: &str = include_str!("../../../fixtures/library-desk.json");

pub struct FixtureCatalogStore {
    state: RwLock<FixtureState>,
}

struct FixtureState {
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
            state: RwLock::new(FixtureState {
                snapshot_version: fixture.snapshot_version,
                skills,
                agents,
            }),
        }
    }
}

impl CatalogStore for FixtureCatalogStore {
    fn snapshot_version(&self) -> u64 {
        self.state
            .read()
            .map(|state| state.snapshot_version)
            .unwrap_or_default()
    }

    fn list(&self, filter: CatalogFilter) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        let state = self
            .state
            .read()
            .map_err(|_| CatalogStoreError::Unavailable("fixture lock poisoned".into()))?;
        Ok(state
            .skills
            .iter()
            .map(|detail| summary_with_count(&state, detail))
            .filter(|skill| filter.includes(skill))
            .collect())
    }

    fn skill_directory_identity_key(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<String>, CatalogStoreError> {
        let state = self.state.read().expect("fixture state lock");
        let Some(skill) = state
            .skills
            .iter()
            .find(|skill| skill.summary.id == *skill_id)
        else {
            return Ok(None);
        };
        Ok(Some(crate::core::domain::skill_identity_key(
            &skill.summary.directory_name,
        )))
    }

    fn inspect(&self, skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError> {
        let state = self
            .state
            .read()
            .map_err(|_| CatalogStoreError::Unavailable("fixture lock poisoned".into()))?;
        Ok(state
            .skills
            .iter()
            .find(|detail| detail.summary.id == *skill_id)
            .map(|detail| detail_with_count(&state, detail)))
    }
}

fn summary_with_count(state: &FixtureState, detail: &SkillDetail) -> SkillSummary {
    let mut summary = detail.summary.clone();
    summary.enabled_agent_count = state
        .agents
        .iter()
        .filter(|agent| agent.enabled_skill_ids.contains(&summary.id.0))
        .count() as u32;
    summary
}

fn detail_with_count(state: &FixtureState, detail: &SkillDetail) -> SkillDetail {
    let mut detail = detail.clone();
    detail.summary = summary_with_count(state, &detail);
    detail
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
    #[serde(default)]
    file_source_original_path: Option<String>,
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
            file_source_original_path: value.file_source_original_path,
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
    enabled_skill_ids: Vec<String>,
}
