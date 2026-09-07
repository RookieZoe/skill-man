use std::path::PathBuf;

use thiserror::Error;

use crate::core::agent_configuration::{AgentConfigurationOrigin, AgentRootRole, Compatibility};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAgentConfiguration {
    pub agent_id: String,
    pub origin: AgentConfigurationOrigin,
    pub preset_key: Option<String>,
    pub name: String,
    pub name_identity_key: String,
    pub compatibility: Compatibility,
    pub project_skills_dir: Option<PathBuf>,
    pub created_at: String,
    pub updated_at: String,
    pub memberships: Vec<StoredAgentRootMembership>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredAgentRootMembership {
    pub root_id: String,
    pub role: AgentRootRole,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredGlobalSkillRoot {
    pub root_id: String,
    pub configured_path: PathBuf,
    pub path_identity_key: String,
    pub consumer_agent_ids: Vec<String>,
    pub activation_skill_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfigurationStoreSnapshot {
    pub snapshot_version: u64,
    pub configurations: Vec<StoredAgentConfiguration>,
    pub roots: Vec<StoredGlobalSkillRoot>,
}

impl AgentConfigurationStoreSnapshot {
    /// Historical activations retain orphan Roots for identity reuse, but
    /// only Roots consumed by a current configuration belong to scan scope.
    pub fn configured_roots(&self) -> impl Iterator<Item = &StoredGlobalSkillRoot> {
        self.roots
            .iter()
            .filter(|root| !root.consumer_agent_ids.is_empty())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfigurationWrite {
    pub agent_id: String,
    pub origin: AgentConfigurationOrigin,
    pub preset_key: Option<String>,
    pub name: String,
    pub name_identity_key: String,
    pub compatibility: Compatibility,
    pub project_skills_dir: Option<PathBuf>,
    pub roots: Vec<AgentRootWrite>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRootWrite {
    pub root_id: String,
    pub configured_path: PathBuf,
    pub path_identity_key: String,
    pub role: AgentRootRole,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentConfigurationStoreChange {
    Create(AgentConfigurationWrite),
    Edit(AgentConfigurationWrite),
    Delete { agent_id: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentProjectFolder {
    pub canonical_path_key: String,
    pub canonical_path: PathBuf,
    pub last_used_at: String,
}

#[derive(Debug, Error)]
pub enum AgentConfigurationStoreError {
    #[error("the Agent Configuration store is unavailable: {0}")]
    Unavailable(String),
    #[error("the Agent Configuration plan is stale")]
    Stale,
    #[error("Agent Configuration '{agent_id}' was not found")]
    NotFound { agent_id: String },
    #[error("an Agent Configuration already uses this name identity")]
    NameConflict,
    #[error("a different Global Skills Root already uses this path identity")]
    RootConflict,
    #[error("the last Target reference still owns Activations")]
    TargetInUse { skill_ids: Vec<String> },
    #[error("the Agent Configuration write violates the one-Target invariant")]
    InvalidTargetMembership,
}

pub trait AgentConfigurationStore: Send + Sync {
    fn agent_configuration_snapshot(
        &self,
    ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError>;

    fn apply_agent_configuration_change(
        &self,
        expected_snapshot_version: u64,
        change: AgentConfigurationStoreChange,
    ) -> Result<u64, AgentConfigurationStoreError>;

    fn list_recent_project_folders(
        &self,
    ) -> Result<Vec<RecentProjectFolder>, AgentConfigurationStoreError>;

    /// Records UI history only after a Project Enable operation reports at
    /// least one successful cell. The Project Enable module owns that call;
    /// this store only enforces the Home-local MRU capacity and ordering.
    fn record_recent_project_folder(
        &self,
        folder: RecentProjectFolder,
    ) -> Result<(), AgentConfigurationStoreError>;

    fn clear_recent_project_folders(&self) -> Result<(), AgentConfigurationStoreError>;
}
