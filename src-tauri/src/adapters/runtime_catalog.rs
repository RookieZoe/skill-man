use std::sync::Arc;

use crate::adapters::fixture_catalog::FixtureCatalogStore;
use crate::adapters::sqlite::SqliteCatalogStore;
use crate::core::domain::{AgentActivation, CatalogFilter, SkillDetail, SkillId, SkillSummary};
use crate::seams::activation_store::{
    ActivationContext, ActivationRecord, ActivationStore, ActivationStoreError, ConfiguredAgentPath,
};
use crate::seams::catalog_store::StartupAccess;
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};

pub struct RuntimeCatalogStore {
    fixture: Arc<FixtureCatalogStore>,
    sqlite: Arc<SqliteCatalogStore>,
}

impl RuntimeCatalogStore {
    pub fn new(fixture: Arc<FixtureCatalogStore>, sqlite: Arc<SqliteCatalogStore>) -> Self {
        Self { fixture, sqlite }
    }

    fn catalog_error(error: ActivationStoreError) -> CatalogStoreError {
        CatalogStoreError::Unavailable(error.to_string())
    }

    fn is_writable(&self) -> bool {
        self.sqlite.startup_status().access == StartupAccess::ReadWrite
    }
}

impl CatalogStore for RuntimeCatalogStore {
    fn snapshot_version(&self) -> u64 {
        if !self.is_writable() {
            return self.fixture.snapshot_version();
        }
        self.sqlite
            .persisted_snapshot_version()
            .unwrap_or_else(|_| self.fixture.snapshot_version())
    }

    fn list(&self, filter: CatalogFilter) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        if !self.is_writable() {
            return self.fixture.list(filter);
        }
        self.fixture
            .list(filter)?
            .into_iter()
            .map(|mut skill| {
                skill.enabled_agent_count = self
                    .sqlite
                    .enabled_count(&skill.id)
                    .map_err(Self::catalog_error)?;
                Ok(skill)
            })
            .collect()
    }

    fn inspect(&self, skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError> {
        if !self.is_writable() {
            return self.fixture.inspect(skill_id);
        }
        let Some(mut detail) = self.fixture.inspect(skill_id)? else {
            return Ok(None);
        };
        detail.summary.enabled_agent_count = self
            .sqlite
            .enabled_count(skill_id)
            .map_err(Self::catalog_error)?;
        Ok(Some(detail))
    }

    fn list_agents(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<Vec<AgentActivation>>, CatalogStoreError> {
        if !self.is_writable() {
            return self.fixture.list_agents(skill_id);
        }
        let Some(agents) = self.fixture.list_agents(skill_id)? else {
            return Ok(None);
        };
        agents
            .into_iter()
            .map(|mut agent| {
                let persisted = self
                    .sqlite
                    .persisted_activation(skill_id, &agent.id)
                    .map_err(Self::catalog_error)?;
                agent.desired_enabled = persisted.is_some_and(|state| state.desired_enabled);
                agent.observed_state = persisted.map_or(
                    crate::core::domain::ActivationObservedState::Missing,
                    |state| state.observed_state,
                );
                Ok(agent)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

impl ActivationStore for RuntimeCatalogStore {
    fn load(
        &self,
        skill_id: &SkillId,
        agent_id: &crate::core::domain::AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.load(skill_id, agent_id)
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.configured_agent_paths()
    }

    fn record(&self, record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.record(record)
    }
}
