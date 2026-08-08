//! Startup module (spec §8.7): first-run detection and the onboarding state
//! the sheet drives. The first launch after install is a first run until the
//! three-step onboarding is finished OR explicitly skipped — skipping still
//! records completion (the app's own bookkeeping; no user content is
//! written), so later launches run the light scan.

use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::core::domain::{AgentId, AgentKind};
use crate::seams::adopt_store::{AdoptStore, AdoptStoreError};
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};
use crate::seams::filesystem::{FileSystem, FileSystemError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupAgent {
    pub agent_id: AgentId,
    pub name: String,
    pub kind: AgentKind,
    pub skills_path: PathBuf,
    pub detected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupInfo {
    pub first_run: bool,
    /// Detected Agent Presets for the onboarding "Preset confirmation" step;
    /// missing directories are marked, never created without confirmation.
    pub agents: Vec<StartupAgent>,
}

#[derive(Clone)]
pub struct StartupService {
    store: Arc<dyn CatalogStore>,
    agents: Arc<dyn AdoptStore>,
    filesystem: Arc<dyn FileSystem>,
}

impl StartupService {
    pub fn new(
        store: Arc<dyn CatalogStore>,
        agents: Arc<dyn AdoptStore>,
        filesystem: Arc<dyn FileSystem>,
    ) -> Self {
        Self {
            store,
            agents,
            filesystem,
        }
    }

    pub fn startup_info(&self) -> Result<StartupInfo, StartupError> {
        let first_run = self.store.first_run_completed_at()?.is_none();
        let agents = self
            .agents
            .list_agents()?
            .into_iter()
            .map(|agent| StartupAgent {
                agent_id: agent.agent_id,
                name: agent.name,
                kind: agent.kind,
                skills_path: agent.skills_path,
                detected: agent.detected,
            })
            .collect();
        Ok(StartupInfo { first_run, agents })
    }

    pub fn complete_onboarding(&self) -> Result<(), StartupError> {
        self.store.mark_first_run_completed()?;
        Ok(())
    }

    /// Create a missing Agent skills directory for a configured Agent Preset
    /// (spec §8.7: missing directories are only marked until the user
    /// explicitly confirms creation). Refuses when the directory already
    /// exists or the Agent is unknown.
    pub fn create_agent_directory(&self, agent_id: &AgentId) -> Result<(), StartupError> {
        let agent = self
            .agents
            .list_agents()?
            .into_iter()
            .find(|agent| &agent.agent_id == agent_id)
            .ok_or_else(|| StartupError::Validation("the Agent is not configured".into()))?;
        if agent.detected {
            return Err(StartupError::Validation(
                "the Agent skills directory already exists".into(),
            ));
        }
        self.filesystem.create_directory(&agent.skills_path)?;
        self.agents.mark_agent_detected(agent_id)?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum StartupError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Store(#[from] CatalogStoreError),
    #[error(transparent)]
    Agents(#[from] AdoptStoreError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
}
