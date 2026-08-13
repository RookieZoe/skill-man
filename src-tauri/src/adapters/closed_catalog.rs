//! Fail-closed store for non-Bound bootstrap states (spec §5.1, §10.1): every
//! catalog-shaped call returns a closed `Unavailable` error so a command
//! accidentally invoked outside `Bound` can never read or write anything.
//! The React surface renders the bootstrap route instead of calling these
//! commands, so this store is defense in depth, not a UI path.

use crate::core::domain::{
    AgentActivation, AgentId, CatalogFilter, SkillDetail, SkillId, SkillSummary,
};
use crate::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedSkillRecord, LibraryConflict as AdoptConflict,
};
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};
use crate::seams::import_store::{
    FileImportRecord, ImportStore, ImportStoreError, LibraryConflict, LinkImportRecord,
    RemoteImportRecord, RemoteInstallRecord,
};
use crate::seams::maintenance_store::{
    AdoptedSkillEntity, InstalledSkillBaseline, LinkSkillRecord, MaintenanceStore,
    MaintenanceStoreError, RelocateActivationBaseline, RemoveTarget, SkillHealthObservation,
};
use crate::seams::preferences_store::{
    AppPreferences, PreferenceUpdates, PreferencesStore, PreferencesStoreError,
};

pub struct ClosedCatalogStore;

impl ClosedCatalogStore {
    fn closed<T>() -> Result<T, CatalogStoreError> {
        Err(CatalogStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }
}

impl CatalogStore for ClosedCatalogStore {
    fn snapshot_version(&self) -> u64 {
        0
    }

    fn list(&self, _filter: CatalogFilter) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        Self::closed()
    }

    fn inspect(&self, _skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError> {
        Self::closed()
    }

    fn list_agents(
        &self,
        _skill_id: &SkillId,
    ) -> Result<Option<Vec<AgentActivation>>, CatalogStoreError> {
        Self::closed()
    }

    fn first_run_completed_at(&self) -> Result<Option<String>, CatalogStoreError> {
        Self::closed()
    }

    fn mark_first_run_completed(&self) -> Result<(), CatalogStoreError> {
        Self::closed()
    }

    fn recently_enabled(&self, _limit: u32) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        Self::closed()
    }
}

impl ImportStore for ClosedCatalogStore {
    fn find_library_conflict(
        &self,
        _identity_key: &str,
    ) -> Result<Option<LibraryConflict>, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn insert_link(&self, _record: LinkImportRecord) -> Result<u64, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn insert_file(&self, _record: FileImportRecord) -> Result<u64, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn insert_files(&self, _records: Vec<FileImportRecord>) -> Result<u64, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn load_file_install(
        &self,
        _identity_key: &str,
    ) -> Result<Option<FileImportRecord>, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn desired_activations_for_skill(
        &self,
        _skill_id: &SkillId,
    ) -> Result<Vec<DesiredActivation>, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn replace_file(&self, _record: FileImportRecord) -> Result<u64, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn insert_remotes(&self, _records: Vec<RemoteImportRecord>) -> Result<u64, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn load_remote_installs(&self) -> Result<Vec<RemoteInstallRecord>, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn load_remote_install(
        &self,
        _identity_key: &str,
    ) -> Result<Option<RemoteInstallRecord>, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn update_remote_install(&self, _record: RemoteImportRecord) -> Result<u64, ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn record_remote_check(&self, _skill_id: &SkillId) -> Result<(), ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn set_remote_requested_ref(
        &self,
        _skill_id: &SkillId,
        _requested_ref: &str,
    ) -> Result<(), ImportStoreError> {
        Err(ImportStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }
}

impl AdoptStore for ClosedCatalogStore {
    fn list_agents(&self) -> Result<Vec<AdoptAgent>, AdoptStoreError> {
        Err(AdoptStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn insert_adopted(&self, _record: AdoptedSkillRecord) -> Result<u64, AdoptStoreError> {
        Err(AdoptStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn remove_adopted_skill(&self, _skill_id: &SkillId) -> Result<u64, AdoptStoreError> {
        Err(AdoptStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn find_library_conflict(
        &self,
        _identity_key: &str,
    ) -> Result<Option<AdoptConflict>, AdoptStoreError> {
        Err(AdoptStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }
}

impl ActivationStore for ClosedCatalogStore {
    fn load(
        &self,
        _skill_id: &SkillId,
        _agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn record(&self, _record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn record_observations(
        &self,
        _observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }
}

impl MaintenanceStore for ClosedCatalogStore {
    fn installed_skill_baselines(
        &self,
    ) -> Result<Vec<InstalledSkillBaseline>, MaintenanceStoreError> {
        Err(MaintenanceStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn link_skill(
        &self,
        _skill_id: &SkillId,
    ) -> Result<Option<LinkSkillRecord>, MaintenanceStoreError> {
        Err(MaintenanceStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn commit_relocate(
        &self,
        _skill_id: &SkillId,
        _final_entity_path: std::path::PathBuf,
        _display_name: String,
        _description: String,
        _new_target_path: std::path::PathBuf,
        _activations: &[RelocateActivationBaseline],
    ) -> Result<u64, MaintenanceStoreError> {
        Err(MaintenanceStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn remove_target(
        &self,
        _skill_id: &SkillId,
    ) -> Result<Option<RemoveTarget>, MaintenanceStoreError> {
        Err(MaintenanceStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn delete_skill(&self, _skill_id: &SkillId) -> Result<u64, MaintenanceStoreError> {
        Err(MaintenanceStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn record_skill_health(
        &self,
        _observations: &[SkillHealthObservation],
    ) -> Result<u64, MaintenanceStoreError> {
        Err(MaintenanceStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn adopted_skill_entities(&self) -> Result<Vec<AdoptedSkillEntity>, MaintenanceStoreError> {
        Err(MaintenanceStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }
}

impl PreferencesStore for ClosedCatalogStore {
    fn load_preferences(&self) -> Result<AppPreferences, PreferencesStoreError> {
        Err(PreferencesStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn update_preferences(
        &self,
        _updates: PreferenceUpdates,
    ) -> Result<AppPreferences, PreferencesStoreError> {
        Err(PreferencesStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn last_app_update_check_at(&self) -> Result<Option<i64>, PreferencesStoreError> {
        Err(PreferencesStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }

    fn record_app_update_check_at(&self, _checked_at: i64) -> Result<(), PreferencesStoreError> {
        Err(PreferencesStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }
}

impl crate::core::activation::ActivationConflictChecker for ClosedCatalogStore {
    fn library_identity_conflict(
        &self,
        _identity_key: &str,
    ) -> Result<Option<AdoptConflict>, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "no Bound Home: the catalog is closed".into(),
        ))
    }
}
