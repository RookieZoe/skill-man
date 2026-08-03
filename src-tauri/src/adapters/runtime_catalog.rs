use std::sync::Arc;

use crate::adapters::fixture_catalog::FixtureCatalogStore;
use crate::adapters::sqlite::SqliteCatalogStore;
use crate::core::domain::{
    AgentActivation, CatalogFilter, SkillDetail, SkillId, SkillSummary, SourceKind,
    parse_skill_metadata,
};
use crate::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use crate::seams::catalog_store::StartupAccess;
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};
use crate::seams::filesystem::FileSystem;
use crate::seams::import_store::{
    ImportStore, ImportStoreError, LibraryConflict, LinkImportRecord,
};

pub struct RuntimeCatalogStore {
    fixture: Arc<FixtureCatalogStore>,
    sqlite: Arc<SqliteCatalogStore>,
    filesystem: Arc<dyn FileSystem>,
}

impl RuntimeCatalogStore {
    pub fn new(
        fixture: Arc<FixtureCatalogStore>,
        sqlite: Arc<SqliteCatalogStore>,
        filesystem: Arc<dyn FileSystem>,
    ) -> Self {
        Self {
            fixture,
            sqlite,
            filesystem,
        }
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
        self.sqlite.list_skill_summaries(filter)
    }

    fn inspect(&self, skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError> {
        if !self.is_writable() {
            return self.fixture.inspect(skill_id);
        }
        let Some(persisted) = self.sqlite.persisted_skill_detail(skill_id)? else {
            return Ok(None);
        };
        if let Some(mut detail) = self.fixture.inspect(skill_id)? {
            detail.summary = persisted.summary;
            return Ok(Some(detail));
        }
        let skill_markdown = self
            .filesystem
            .read_skill_document(&persisted.final_entity_path)
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))?;
        let metadata = parse_skill_metadata(&skill_markdown);
        let source_label = match persisted.summary.source_kind {
            SourceKind::Link => format!(
                "Linked local folder · {}",
                persisted.final_entity_path.display()
            ),
            SourceKind::RemoteInstall => "Installed from Git".into(),
            SourceKind::FileInstall => "Installed from file".into(),
        };
        Ok(Some(SkillDetail {
            summary: persisted.summary,
            final_entity_path: persisted.final_entity_path.to_string_lossy().into_owned(),
            source_label,
            frontmatter_name: metadata.name,
            last_activity_at: persisted.updated_at,
            skill_markdown,
        }))
    }

    fn list_agents(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<Vec<AgentActivation>>, CatalogStoreError> {
        if !self.is_writable() {
            return self.fixture.list_agents(skill_id);
        }
        self.sqlite.list_agent_activations(skill_id)
    }
}

impl ImportStore for RuntimeCatalogStore {
    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<LibraryConflict>, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.find_library_conflict(identity_key)
    }

    fn insert_link(&self, record: LinkImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.insert_link(record)
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

    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.desired_activations()
    }

    fn record_observations(
        &self,
        observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.record_observations(observations)
    }

    fn record_observation(
        &self,
        observation: &ActivationObservation,
    ) -> Result<u64, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.record_observation(observation)
    }
}
