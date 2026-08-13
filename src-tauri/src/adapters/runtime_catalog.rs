use std::path::PathBuf;
use std::sync::Arc;

use crate::adapters::sqlite::SqliteCatalogStore;
use crate::core::domain::{
    AgentActivation, AgentId, CatalogFilter, Health, SkillDetail, SkillId, SkillSummary,
    parse_skill_metadata,
};
use crate::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedSkillRecord, LibraryConflict as AdoptConflict,
};
use crate::seams::catalog_store::StartupAccess;
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};
use crate::seams::filesystem::{ActivationRecoveryBaseline, FileSystem};
use crate::seams::import_store::{
    FileImportRecord, ImportStore, ImportStoreError, LibraryConflict, LinkImportRecord,
    RemoteImportRecord, RemoteInstallRecord,
};
use crate::seams::maintenance_store::{
    AdoptedSkillEntity, InstalledSkillBaseline, LinkSkillRecord, MaintenanceStore,
    MaintenanceStoreError, ManagedSkillBaseline, RelocateActivationBaseline, RemoveTarget,
    SkillHealthObservation,
};

pub struct RuntimeCatalogStore {
    sqlite: Arc<SqliteCatalogStore>,
    filesystem: Arc<dyn FileSystem>,
}

impl RuntimeCatalogStore {
    pub fn new(sqlite: Arc<SqliteCatalogStore>, filesystem: Arc<dyn FileSystem>) -> Self {
        Self { sqlite, filesystem }
    }

    fn is_writable(&self) -> bool {
        self.sqlite.startup_status().access == StartupAccess::ReadWrite
    }
}

impl CatalogStore for RuntimeCatalogStore {
    fn snapshot_version(&self) -> u64 {
        self.sqlite.persisted_snapshot_version().unwrap_or(0)
    }

    fn list(&self, filter: CatalogFilter) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        self.sqlite.list_skill_summaries(filter)
    }

    fn inspect(&self, skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError> {
        let Some(persisted) = self.sqlite.persisted_skill_detail(skill_id)? else {
            return Ok(None);
        };
        let skill_markdown = match self
            .filesystem
            .read_skill_document(&persisted.final_entity_path)
        {
            Ok(markdown) => markdown,
            // A Broken Skill (missing entity or SKILL.md) must stay viewable:
            // the detail panel presents the notice and repair entry instead of
            // failing the whole inspection.
            Err(_) if persisted.summary.health == Health::Broken => String::new(),
            Err(error) => {
                return Err(CatalogStoreError::Unavailable(error.to_string()));
            }
        };
        let metadata = parse_skill_metadata(&skill_markdown);
        Ok(Some(SkillDetail {
            summary: persisted.summary,
            final_entity_path: persisted.final_entity_path.to_string_lossy().into_owned(),
            file_source_original_path: persisted.file_source_original_path,
            frontmatter_name: metadata.name,
            last_activity_at: persisted.updated_at,
            skill_markdown,
        }))
    }

    fn list_agents(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<Vec<AgentActivation>>, CatalogStoreError> {
        self.sqlite.list_agent_activations(skill_id)
    }

    fn first_run_completed_at(&self) -> Result<Option<String>, CatalogStoreError> {
        if !self.is_writable() {
            // A read-only catalog must not masquerade as a first run: the
            // locked state is an error surfaced elsewhere, not onboarding.
            return Err(CatalogStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.first_run_completed_at()
    }

    fn mark_first_run_completed(&self) -> Result<(), CatalogStoreError> {
        if !self.is_writable() {
            return Err(CatalogStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.mark_first_run_completed()
    }

    fn recently_enabled(&self, limit: u32) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        if !self.is_writable() {
            return Ok(Vec::new());
        }
        self.sqlite.recently_enabled_skills(limit)
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
        crate::seams::import_store::ImportStore::find_library_conflict(
            self.sqlite.as_ref(),
            identity_key,
        )
    }

    fn insert_link(&self, record: LinkImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.insert_link(record)
    }

    fn insert_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.insert_file(record)
    }

    fn insert_files(&self, records: Vec<FileImportRecord>) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.insert_files(records)
    }

    fn load_file_install(
        &self,
        identity_key: &str,
    ) -> Result<Option<FileImportRecord>, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.load_file_install(identity_key)
    }

    fn desired_activations_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<DesiredActivation>, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.desired_activations_for_skill(skill_id)
    }

    fn replace_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.replace_file(record)
    }

    fn insert_remotes(&self, records: Vec<RemoteImportRecord>) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.insert_remotes(records)
    }

    fn load_remote_installs(&self) -> Result<Vec<RemoteInstallRecord>, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.load_remote_installs()
    }

    fn load_remote_install(
        &self,
        identity_key: &str,
    ) -> Result<Option<RemoteInstallRecord>, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.load_remote_install(identity_key)
    }

    fn update_remote_install(&self, record: RemoteImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.update_remote_install(record)
    }

    fn record_remote_check(&self, skill_id: &SkillId) -> Result<(), ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.record_remote_check(skill_id)
    }

    fn set_remote_requested_ref(
        &self,
        skill_id: &SkillId,
        requested_ref: &str,
    ) -> Result<(), ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite
            .set_remote_requested_ref(skill_id, requested_ref)
    }
}

impl AdoptStore for RuntimeCatalogStore {
    fn list_agents(&self) -> Result<Vec<AdoptAgent>, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.list_agents()
    }

    fn mark_agent_detected(&self, agent_id: &AgentId) -> Result<(), AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.mark_agent_detected(agent_id)
    }

    fn insert_adopted(&self, record: AdoptedSkillRecord) -> Result<u64, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.insert_adopted(record)
    }

    fn remove_adopted_skill(&self, skill_id: &SkillId) -> Result<u64, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.remove_adopted_skill(skill_id)
    }

    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<AdoptConflict>, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        crate::seams::adopt_store::AdoptStore::find_library_conflict(
            self.sqlite.as_ref(),
            identity_key,
        )
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

impl MaintenanceStore for RuntimeCatalogStore {
    fn adopted_skill_entities(&self) -> Result<Vec<AdoptedSkillEntity>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; Adopt recovery entities are unavailable".into(),
            ));
        }
        self.sqlite.adopted_skill_entities()
    }

    fn installed_skill_baselines(
        &self,
    ) -> Result<Vec<InstalledSkillBaseline>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; recovery baselines are unavailable".into(),
            ));
        }
        self.sqlite.installed_skill_baselines()
    }

    fn record_skill_health(
        &self,
        observations: &[SkillHealthObservation],
    ) -> Result<u64, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.record_skill_health(observations)
    }

    fn managed_skill_baselines(&self) -> Result<Vec<ManagedSkillBaseline>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; health baselines are unavailable".into(),
            ));
        }
        self.sqlite.managed_skill_baselines()
    }

    fn link_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<LinkSkillRecord>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.link_skill(skill_id)
    }

    fn activation_baselines_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<RelocateActivationBaseline>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.activation_baselines_for_skill(skill_id)
    }

    fn commit_relocate(
        &self,
        skill_id: &SkillId,
        final_entity_path: PathBuf,
        display_name: String,
        description: String,
        new_target_path: PathBuf,
        activations: &[RelocateActivationBaseline],
    ) -> Result<u64, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.commit_relocate(
            skill_id,
            final_entity_path,
            display_name,
            description,
            new_target_path,
            activations,
        )
    }

    fn remove_target(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<RemoveTarget>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.remove_target(skill_id)
    }

    fn delete_skill(&self, skill_id: &SkillId) -> Result<u64, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.sqlite.delete_skill(skill_id)
    }

    fn desired_activation_baselines(
        &self,
    ) -> Result<Vec<ActivationRecoveryBaseline>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; Activation replace recovery baselines are unavailable"
                    .into(),
            ));
        }
        crate::seams::activation_store::ActivationStore::desired_activations(self.sqlite.as_ref())
            .map(|activations| {
                activations
                    .into_iter()
                    .map(|activation| ActivationRecoveryBaseline {
                        skill_id: activation.skill_id.0,
                        agent_id: activation.agent_id.0,
                        expected_entry_path: activation.expected_entry_path,
                        expected_target_path: activation.expected_target_path,
                    })
                    .collect()
            })
            .map_err(|error| MaintenanceStoreError::Unavailable(error.to_string()))
    }
}

impl crate::seams::preferences_store::PreferencesStore for RuntimeCatalogStore {
    fn load_preferences(
        &self,
    ) -> Result<
        crate::seams::preferences_store::AppPreferences,
        crate::seams::preferences_store::PreferencesStoreError,
    > {
        if !self.is_writable() {
            return Err(
                crate::seams::preferences_store::PreferencesStoreError::Unavailable(
                    "catalog startup is read-only".into(),
                ),
            );
        }
        self.sqlite.load_preferences()
    }

    fn update_preferences(
        &self,
        updates: crate::seams::preferences_store::PreferenceUpdates,
    ) -> Result<
        crate::seams::preferences_store::AppPreferences,
        crate::seams::preferences_store::PreferencesStoreError,
    > {
        if !self.is_writable() {
            return Err(
                crate::seams::preferences_store::PreferencesStoreError::Unavailable(
                    "catalog startup is read-only".into(),
                ),
            );
        }
        self.sqlite.update_preferences(updates)
    }

    fn last_app_update_check_at(
        &self,
    ) -> Result<Option<i64>, crate::seams::preferences_store::PreferencesStoreError> {
        if !self.is_writable() {
            return Err(
                crate::seams::preferences_store::PreferencesStoreError::Unavailable(
                    "catalog startup is read-only".into(),
                ),
            );
        }
        self.sqlite.last_app_update_check_at()
    }

    fn record_app_update_check_at(
        &self,
        checked_at: i64,
    ) -> Result<(), crate::seams::preferences_store::PreferencesStoreError> {
        if !self.is_writable() {
            return Err(
                crate::seams::preferences_store::PreferencesStoreError::Unavailable(
                    "catalog startup is read-only".into(),
                ),
            );
        }
        self.sqlite.record_app_update_check_at(checked_at)
    }
}

impl crate::core::activation::ActivationConflictChecker for RuntimeCatalogStore {
    fn library_identity_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<AdoptConflict>, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        crate::seams::adopt_store::AdoptStore::find_library_conflict(
            self.sqlite.as_ref(),
            identity_key,
        )
        .map_err(|error| ActivationStoreError::Unavailable(error.to_string()))
    }
}
