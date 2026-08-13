use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use serde::Deserialize;

use crate::core::domain::{
    ActivationObservedState, AgentActivation, AgentId, AgentKind, CatalogFilter, Compatibility,
    Health, SkillDetail, SkillId, SkillSummary, SourceKind,
};
use crate::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};

const LIBRARY_DESK_FIXTURE: &str = include_str!("../../../fixtures/library-desk.json");

pub struct FixtureCatalogStore {
    state: RwLock<FixtureState>,
}

struct FixtureState {
    snapshot_version: u64,
    skills: Vec<SkillDetail>,
    agents: Vec<FixtureAgent>,
    expected_targets: HashMap<(String, String), PathBuf>,
    observed_states: HashMap<(String, String), ActivationObservedState>,
}

impl FixtureCatalogStore {
    pub fn library_desk() -> Self {
        let fixture: FixtureFile =
            serde_json::from_str(LIBRARY_DESK_FIXTURE).expect("valid Library Desk fixture");
        let agents = fixture.agents;
        let mut skills: Vec<SkillDetail> =
            fixture.skills.into_iter().map(SkillDetail::from).collect();
        let mut expected_targets = HashMap::new();
        let mut observed_states = HashMap::new();
        for skill in &mut skills {
            skill.summary.enabled_agent_count = agents
                .iter()
                .filter(|agent| agent.enabled_skill_ids.contains(&skill.summary.id.0))
                .count() as u32;
            for agent in &agents {
                if agent.enabled_skill_ids.contains(&skill.summary.id.0) {
                    let key = (skill.summary.id.0.clone(), agent.id.clone());
                    expected_targets.insert(key.clone(), PathBuf::from(&skill.final_entity_path));
                    observed_states.insert(
                        key,
                        agent
                            .observed_skill_states
                            .get(&skill.summary.id.0)
                            .copied()
                            .map(ActivationObservedState::from)
                            .unwrap_or(ActivationObservedState::Present),
                    );
                }
            }
        }
        Self {
            state: RwLock::new(FixtureState {
                snapshot_version: fixture.snapshot_version,
                skills,
                agents,
                expected_targets,
                observed_states,
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

    fn list_agents(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<Vec<AgentActivation>>, CatalogStoreError> {
        let state = self
            .state
            .read()
            .map_err(|_| CatalogStoreError::Unavailable("fixture lock poisoned".into()))?;
        if !state
            .skills
            .iter()
            .any(|detail| detail.summary.id == *skill_id)
        {
            return Ok(None);
        }

        Ok(Some(
            state
                .agents
                .iter()
                .map(|agent| agent.activation_for(&state, skill_id))
                .collect(),
        ))
    }
}

impl ActivationStore for FixtureCatalogStore {
    fn load(
        &self,
        skill_id: &SkillId,
        agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        let state = self
            .state
            .read()
            .map_err(|_| ActivationStoreError::Unavailable("fixture lock poisoned".into()))?;
        let Some(skill) = state
            .skills
            .iter()
            .find(|skill| skill.summary.id == *skill_id)
        else {
            return Ok(None);
        };
        let Some(agent) = state.agents.iter().find(|agent| agent.id == agent_id.0) else {
            return Ok(None);
        };
        let key = (skill_id.0.clone(), agent_id.0.clone());
        Ok(Some(ActivationContext {
            skill_id: skill_id.clone(),
            directory_name: skill.summary.directory_name.clone(),
            final_entity_path: PathBuf::from(&skill.final_entity_path),
            agent_id: agent_id.clone(),
            agent_name: agent.name.clone(),
            agent_kind: agent.kind.into(),
            agent_skills_path: PathBuf::from(&agent.skills_path),
            desired_enabled: agent.enabled_skill_ids.contains(&skill_id.0),
            expected_target_path: state.expected_targets.get(&key).cloned(),
        }))
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        let state = self
            .state
            .read()
            .map_err(|_| ActivationStoreError::Unavailable("fixture lock poisoned".into()))?;
        Ok(state
            .agents
            .iter()
            .map(|agent| ConfiguredAgentPath {
                agent_id: AgentId(agent.id.clone()),
                skills_path: PathBuf::from(&agent.skills_path),
            })
            .collect())
    }

    fn record(&self, record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ActivationStoreError::Unavailable("fixture lock poisoned".into()))?;
        let Some(agent) = state
            .agents
            .iter_mut()
            .find(|agent| agent.id == record.agent_id.0)
        else {
            return Err(ActivationStoreError::Unavailable(
                "fixture Agent was not found".into(),
            ));
        };
        if record.desired_enabled {
            if !agent.enabled_skill_ids.contains(&record.skill_id.0) {
                agent.enabled_skill_ids.push(record.skill_id.0.clone());
            }
        } else {
            agent
                .enabled_skill_ids
                .retain(|skill_id| skill_id != &record.skill_id.0);
        }
        let key = (record.skill_id.0, record.agent_id.0);
        state
            .expected_targets
            .insert(key.clone(), record.expected_target_path);
        state.observed_states.insert(key, record.observed_state);
        state.snapshot_version += 1;
        Ok(state.snapshot_version)
    }

    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        let state = self
            .state
            .read()
            .map_err(|_| ActivationStoreError::Unavailable("fixture lock poisoned".into()))?;
        let mut desired = Vec::new();
        for skill in &state.skills {
            for agent in &state.agents {
                let key = (skill.summary.id.0.clone(), agent.id.clone());
                if !agent.enabled_skill_ids.contains(&skill.summary.id.0) {
                    continue;
                }
                if let Some(expected_target_path) = state.expected_targets.get(&key) {
                    desired.push(DesiredActivation {
                        skill_id: skill.summary.id.clone(),
                        agent_id: AgentId(agent.id.clone()),
                        expected_entry_path: PathBuf::from(&agent.skills_path)
                            .join(&skill.summary.directory_name),
                        expected_target_path: expected_target_path.clone(),
                    });
                }
            }
        }
        Ok(desired)
    }

    fn record_observations(
        &self,
        observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| ActivationStoreError::Unavailable("fixture lock poisoned".into()))?;
        for observation in observations {
            state.observed_states.insert(
                (
                    observation.skill_id.0.clone(),
                    observation.agent_id.0.clone(),
                ),
                observation.observed_state,
            );
        }
        state.snapshot_version += 1;
        Ok(state.snapshot_version)
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
    #[serde(default)]
    observed_skill_states: HashMap<String, FixtureActivationObservedState>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixtureActivationObservedState {
    Present,
    Missing,
    TargetMismatch,
    Dangling,
    Occupied,
}

impl From<FixtureActivationObservedState> for ActivationObservedState {
    fn from(value: FixtureActivationObservedState) -> Self {
        match value {
            FixtureActivationObservedState::Present => Self::Present,
            FixtureActivationObservedState::Missing => Self::Missing,
            FixtureActivationObservedState::TargetMismatch => Self::TargetMismatch,
            FixtureActivationObservedState::Dangling => Self::Dangling,
            FixtureActivationObservedState::Occupied => Self::Occupied,
        }
    }
}

impl FixtureAgent {
    fn activation_for(&self, state: &FixtureState, skill_id: &SkillId) -> AgentActivation {
        let desired_enabled = self.enabled_skill_ids.contains(&skill_id.0);
        let key = (skill_id.0.clone(), self.id.clone());
        AgentActivation {
            id: AgentId(self.id.clone()),
            name: self.name.clone(),
            kind: self.kind.into(),
            skills_path: self.skills_path.clone(),
            detected: self.detected,
            compatibility: self.compatibility.into(),
            desired_enabled,
            observed_state: state.observed_states.get(&key).copied().unwrap_or(
                if desired_enabled {
                    ActivationObservedState::Present
                } else {
                    ActivationObservedState::Missing
                },
            ),
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
