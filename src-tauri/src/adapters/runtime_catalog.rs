use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::adapters::sqlite::SqliteCatalogStore;
use crate::core::bootstrap::{BootstrapService, BootstrapSnapshot, CatalogAccess};
use crate::core::domain::{
    AgentActivation, AgentId, CatalogFilter, Health, SkillDetail, SkillId, SkillSummary,
    parse_skill_metadata,
};
use crate::core::home::BoundHome;
use crate::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedSkillRecord, LibraryConflict as AdoptConflict,
    RemoteAdoptedSkillRecord,
};
use crate::seams::catalog_store::StartupAccess;
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};
use crate::seams::filesystem::{ActivationRecoveryBaseline, FileSystem};
use crate::seams::import_store::{
    FileImportRecord, ImportStore, ImportStoreError, LibraryConflict, LinkImportRecord,
    RemoteImportRecord, RemoteInstallRecord, RemoteParentRecord,
};
use crate::seams::maintenance_store::{
    AdoptedSkillEntity, HandoffRecoveredRecord, InstalledSkillBaseline, LinkSkillRecord,
    MaintenanceStore, MaintenanceStoreError, ManagedSkillBaseline, RelocateActivationBaseline,
    RemoveTarget, SkillHealthObservation,
};

pub struct RuntimeCatalogStore {
    /// The current SQLite store; `None` means the catalog is closed for
    /// this bootstrap state. Swappable so Reconnect / Restore / Abandon can
    /// reopen or close the same facade every service already holds.
    sqlite: RwLock<Option<Arc<SqliteCatalogStore>>>,
    filesystem: Arc<dyn FileSystem>,
}

/// Aligns the shared store facade with a fresh bootstrap snapshot: a Bound
/// ReadWrite result reopens the Catalog writable, a Bound read-only result
/// reopens it read-only, every other state closes the facade. Reconnect,
/// Restore commit and Abandon all route through this switch, so product
/// writes reopen or close without rebuilding the service graph.
pub struct RuntimeStoreSwitch {
    store: Arc<RuntimeCatalogStore>,
    catalog_file_name: String,
}

impl RuntimeStoreSwitch {
    pub fn new(store: Arc<RuntimeCatalogStore>, catalog_file_name: String) -> Self {
        Self {
            store,
            catalog_file_name,
        }
    }

    pub fn reconcile(
        &self,
        snapshot: &BootstrapSnapshot,
        bound_home: Option<&BoundHome>,
    ) -> Result<(), String> {
        match (snapshot, bound_home) {
            (
                BootstrapSnapshot::Bound {
                    catalog_access: CatalogAccess::ReadWrite,
                    ..
                },
                Some(home),
            ) => self
                .store
                .reopen_bound(home, &home.path.join(&self.catalog_file_name)),
            (
                BootstrapSnapshot::Bound {
                    catalog_access: CatalogAccess::ReadOnly { .. },
                    ..
                },
                Some(home),
            ) => self
                .store
                .reopen_read_only(&home.path.join(&self.catalog_file_name)),
            _ => {
                self.store.replace_store(None);
                Ok(())
            }
        }
    }

    /// Align the facade after a lifecycle transition (Reconnect success,
    /// Restore commit): a fresh `inspect` clears a stale writable-open
    /// failure and recomputes the true catalog access, then the facade is
    /// reopened writable, read-only or closed to match.
    pub fn reconcile_after_transition(
        &self,
        bootstrap: &BootstrapService,
        snapshot: &BootstrapSnapshot,
    ) -> Result<(), String> {
        if let BootstrapSnapshot::Bound { .. } = snapshot {
            bootstrap.clear_catalog_open_failure();
        }
        let fresh = bootstrap.inspect();
        let bound_home = bootstrap.verified_bound_home();
        self.reconcile(&fresh, bound_home.as_ref())
    }
}

impl RuntimeCatalogStore {
    pub fn new(sqlite: Arc<SqliteCatalogStore>, filesystem: Arc<dyn FileSystem>) -> Self {
        Self {
            sqlite: RwLock::new(Some(sqlite)),
            filesystem,
        }
    }

    /// The fail-closed facade for non-Bound bootstrap states: every
    /// catalog-shaped call behaves like `ClosedCatalogStore`.
    pub fn closed(filesystem: Arc<dyn FileSystem>) -> Self {
        Self {
            sqlite: RwLock::new(None),
            filesystem,
        }
    }

    /// The current store, or `None` when closed.
    pub fn store(&self) -> Option<Arc<SqliteCatalogStore>> {
        self.sqlite.read().ok().and_then(|slot| slot.clone())
    }

    /// Swap the store (Reconnect / Restore success, Abandon close).
    pub fn replace_store(&self, sqlite: Option<Arc<SqliteCatalogStore>>) {
        if let Ok(mut slot) = self.sqlite.write() {
            *slot = sqlite;
        }
    }

    /// Reopen the verified Bound Home's Catalog for writing (Reconnect /
    /// Restore commit). Identity is re-verified by `open_bound`; on failure
    /// the current store is left untouched.
    pub fn reopen_bound(
        &self,
        home: &BoundHome,
        catalog_path: &std::path::Path,
    ) -> Result<(), String> {
        let sqlite = SqliteCatalogStore::open_bound(home, catalog_path)
            .map_err(|error| error.to_string())?;
        self.replace_store(Some(Arc::new(sqlite)));
        Ok(())
    }

    /// Reopen a Catalog read-only (a Bound Home whose writable open must
    /// not happen, e.g. integrity-failed content awaiting Restore).
    pub fn reopen_read_only(&self, catalog_path: &std::path::Path) -> Result<(), String> {
        let sqlite =
            SqliteCatalogStore::open_read_only(catalog_path).map_err(|error| error.to_string())?;
        self.replace_store(Some(Arc::new(sqlite)));
        Ok(())
    }

    fn is_writable(&self) -> bool {
        self.store()
            .is_some_and(|sqlite| sqlite.startup_status().access == StartupAccess::ReadWrite)
    }

    /// The current store; callers must have verified `is_writable` or the
    /// closed path themselves.
    fn require(&self) -> Arc<SqliteCatalogStore> {
        self.store().expect("the store is present while writable")
    }

    /// The current store or the closed `CatalogStore` error — the fail-closed
    /// path for read commands outside `Bound`.
    fn require_catalog(&self) -> Result<Arc<SqliteCatalogStore>, CatalogStoreError> {
        self.store().ok_or_else(|| {
            CatalogStoreError::Unavailable("no Bound Home: the catalog is closed".into())
        })
    }
}

impl CatalogStore for RuntimeCatalogStore {
    fn snapshot_version(&self) -> u64 {
        match self.store() {
            Some(sqlite) => sqlite.persisted_snapshot_version().unwrap_or(0),
            None => 0,
        }
    }

    fn list(&self, filter: CatalogFilter) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        self.require_catalog()?.list_skill_summaries(filter)
    }

    fn inspect(&self, skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError> {
        let Some(persisted) = self.require_catalog()?.persisted_skill_detail(skill_id)? else {
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
        self.require_catalog()?.list_agent_activations(skill_id)
    }

    fn first_run_completed_at(&self) -> Result<Option<String>, CatalogStoreError> {
        if !self.is_writable() {
            // A read-only catalog must not masquerade as a first run: the
            // locked state is an error surfaced elsewhere, not onboarding.
            return Err(CatalogStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().first_run_completed_at()
    }

    fn mark_first_run_completed(&self) -> Result<(), CatalogStoreError> {
        if !self.is_writable() {
            return Err(CatalogStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().mark_first_run_completed()
    }

    fn recently_enabled(&self, limit: u32) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        if !self.is_writable() {
            return Ok(Vec::new());
        }
        self.require().recently_enabled_skills(limit)
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
            self.require().as_ref(),
            identity_key,
        )
    }

    fn insert_link(&self, record: LinkImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().insert_link(record)
    }

    fn insert_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().insert_file(record)
    }

    fn insert_files(&self, records: Vec<FileImportRecord>) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().insert_files(records)
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
        self.require().load_file_install(identity_key)
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
        self.require().desired_activations_for_skill(skill_id)
    }

    fn replace_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().replace_file(record)
    }

    fn insert_remotes(&self, records: Vec<RemoteImportRecord>) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().insert_remotes(records)
    }

    fn load_remote_installs(&self) -> Result<Vec<RemoteInstallRecord>, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().load_remote_installs()
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
        self.require().load_remote_install(identity_key)
    }

    fn update_remote_install(&self, record: RemoteImportRecord) -> Result<u64, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().update_remote_install(record)
    }

    fn record_remote_check(&self, skill_id: &SkillId) -> Result<(), ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().record_remote_check(skill_id)
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
        self.require()
            .set_remote_requested_ref(skill_id, requested_ref)
    }

    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        crate::seams::import_store::ImportStore::find_remote_parent_by_url(
            self.require().as_ref(),
            canonical_url,
        )
    }

    fn load_remote_parents(&self) -> Result<Vec<RemoteParentRecord>, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().load_remote_parents()
    }

    fn insert_remote_alias(
        &self,
        remote_id: &str,
        alias_url: &str,
    ) -> Result<(), ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().insert_remote_alias(remote_id, alias_url)
    }

    fn delete_remote_parent_if_last_child(
        &self,
        remote_id: &str,
    ) -> Result<bool, ImportStoreError> {
        if !self.is_writable() {
            return Err(ImportStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        crate::seams::import_store::ImportStore::delete_remote_parent_if_last_child(
            self.require().as_ref(),
            remote_id,
        )
    }
}

impl AdoptStore for RuntimeCatalogStore {
    fn list_agents(&self) -> Result<Vec<AdoptAgent>, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().list_agents()
    }

    fn mark_agent_detected(&self, agent_id: &AgentId) -> Result<(), AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().mark_agent_detected(agent_id)
    }

    fn insert_adopted(&self, record: AdoptedSkillRecord) -> Result<u64, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().insert_adopted(record)
    }

    fn remove_adopted_skill(&self, skill_id: &SkillId) -> Result<u64, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().remove_adopted_skill(skill_id)
    }

    fn insert_remote_adopted(
        &self,
        record: RemoteAdoptedSkillRecord,
    ) -> Result<u64, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().insert_remote_adopted(record)
    }

    fn delete_remote_parent_if_last_child(&self, remote_id: &str) -> Result<bool, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        crate::seams::adopt_store::AdoptStore::delete_remote_parent_if_last_child(
            self.require().as_ref(),
            remote_id,
        )
    }

    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        crate::seams::adopt_store::AdoptStore::find_remote_parent_by_url(
            self.require().as_ref(),
            canonical_url,
        )
    }

    fn binding_remote_id(&self, skill_id: &SkillId) -> Result<Option<String>, AdoptStoreError> {
        if !self.is_writable() {
            return Err(AdoptStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        crate::seams::adopt_store::AdoptStore::binding_remote_id(self.require().as_ref(), skill_id)
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
            self.require().as_ref(),
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
        self.require().load(skill_id, agent_id)
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().configured_agent_paths()
    }

    fn record(&self, record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().record(record)
    }

    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        if !self.is_writable() {
            return Err(ActivationStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().desired_activations()
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
        self.require().record_observations(observations)
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
        self.require().record_observation(observation)
    }
}

impl MaintenanceStore for RuntimeCatalogStore {
    fn adopted_skill_entities(&self) -> Result<Vec<AdoptedSkillEntity>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; Adopt recovery entities are unavailable".into(),
            ));
        }
        self.require().adopted_skill_entities()
    }

    fn installed_skill_baselines(
        &self,
    ) -> Result<Vec<InstalledSkillBaseline>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; recovery baselines are unavailable".into(),
            ));
        }
        self.require().installed_skill_baselines()
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
        self.require().record_skill_health(observations)
    }

    fn managed_skill_baselines(&self) -> Result<Vec<ManagedSkillBaseline>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; health baselines are unavailable".into(),
            ));
        }
        self.require().managed_skill_baselines()
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
        self.require().link_skill(skill_id)
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
        self.require().activation_baselines_for_skill(skill_id)
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
        self.require().commit_relocate(
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
        self.require().remove_target(skill_id)
    }

    fn delete_skill(&self, skill_id: &SkillId) -> Result<u64, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().delete_skill(skill_id)
    }

    fn binding_remote_id(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<String>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().binding_remote_id(skill_id)
    }

    fn delete_remote_parent_if_last_child(
        &self,
        remote_id: &str,
    ) -> Result<bool, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().delete_remote_parent_if_last_child(remote_id)
    }

    fn insert_handoff_recovered(
        &self,
        record: HandoffRecoveredRecord,
    ) -> Result<u64, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        self.require().insert_handoff_recovered(record)
    }

    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, MaintenanceStoreError> {
        if !self.is_writable() {
            return Err(MaintenanceStoreError::Unavailable(
                "catalog startup is read-only".into(),
            ));
        }
        crate::seams::import_store::ImportStore::find_remote_parent_by_url(
            self.require().as_ref(),
            canonical_url,
        )
        .map_err(|error| MaintenanceStoreError::Unavailable(error.to_string()))
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
        crate::seams::activation_store::ActivationStore::desired_activations(
            self.require().as_ref(),
        )
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
        self.require().load_preferences()
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
        self.require().update_preferences(updates)
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
        self.require().last_app_update_check_at()
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
        self.require().record_app_update_check_at(checked_at)
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
            self.require().as_ref(),
            identity_key,
        )
        .map_err(|error| ActivationStoreError::Unavailable(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::adapters::macos_fs::MacOsFileSystem;
    use crate::adapters::sqlite::SqliteCatalogStore;
    use crate::core::bootstrap::{BootstrapSnapshot, CatalogAccess};
    use crate::core::home::BoundHome;
    use crate::core::write_gate::ReadOnlyReason;
    use crate::seams::catalog_store::CatalogStoreError;

    use super::*;

    const HOME_ID: &str = "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";

    fn bound_home(path: std::path::PathBuf) -> BoundHome {
        BoundHome {
            home_id: crate::core::home::HomeId(HOME_ID.into()),
            path,
            volume_fsid: "test-fsid".into(),
            volume_uuid: "test-uuid".into(),
            bound_at: "2026-08-01T00:00:00Z".into(),
        }
    }

    fn open_store(dir: &std::path::Path) -> (Arc<RuntimeCatalogStore>, BoundHome) {
        let home = bound_home(dir.join("home"));
        std::fs::create_dir_all(&home.path).expect("home dir");
        let sqlite = SqliteCatalogStore::create_bound(&home, &home.path.join("skill-man.sqlite3"))
            .expect("create bound Catalog");
        let store = Arc::new(RuntimeCatalogStore::new(
            Arc::new(sqlite),
            Arc::new(MacOsFileSystem::new(dir.to_path_buf())),
        ));
        (store, home)
    }

    #[test]
    fn closed_facade_fails_every_catalog_shape_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store =
            RuntimeCatalogStore::closed(Arc::new(MacOsFileSystem::new(dir.path().to_path_buf())));
        assert_eq!(store.snapshot_version(), 0);
        assert!(matches!(
            store.list(CatalogFilter::All),
            Err(CatalogStoreError::Unavailable(_))
        ));
        assert!(!store.is_writable());
        assert!(store.store().is_none());
    }

    #[test]
    fn replace_store_swaps_the_facade_for_reconnect_and_abandon() {
        let dir = tempfile::tempdir().expect("temp dir");
        let filesystem = Arc::new(MacOsFileSystem::new(dir.path().to_path_buf()));
        let store = Arc::new(RuntimeCatalogStore::closed(filesystem.clone()));
        assert!(store.store().is_none());

        let (_, home) = open_store(dir.path());
        store
            .reopen_bound(&home, &home.path.join("skill-man.sqlite3"))
            .expect("reopen bound");
        assert!(store.store().is_some());
        assert!(store.is_writable());
        assert_eq!(
            store.list(CatalogFilter::All).expect("list").len(),
            0,
            "the fresh Catalog is empty"
        );

        // Abandon closes the same facade.
        store.replace_store(None);
        assert!(store.store().is_none());
        assert!(matches!(
            store.list(CatalogFilter::All),
            Err(CatalogStoreError::Unavailable(_))
        ));
    }

    #[test]
    fn store_switch_reconciles_snapshot_to_open_or_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (store, home) = open_store(dir.path());
        let switch = RuntimeStoreSwitch::new(store.clone(), "skill-man.sqlite3".into());

        // Bound ReadWrite: the facade reopens writable.
        switch
            .reconcile(
                &BootstrapSnapshot::Bound {
                    home_id: home.home_id.clone(),
                    catalog_access: CatalogAccess::ReadWrite,
                    snapshot_version: 1,
                },
                Some(&home),
            )
            .expect("reconcile Bound");
        assert!(store.is_writable());

        // Bound read-only (integrity-failed content awaiting Restore): the
        // facade reopens read-only.
        switch
            .reconcile(
                &BootstrapSnapshot::Bound {
                    home_id: home.home_id.clone(),
                    catalog_access: CatalogAccess::ReadOnly {
                        reason: ReadOnlyReason::IntegrityFailed,
                    },
                    snapshot_version: 1,
                },
                Some(&home),
            )
            .expect("reconcile Bound read-only");
        assert!(!store.is_writable(), "read-only reopen never writes");
        assert!(store.store().is_some(), "reads stay available");

        // Unconfigured / Abandoned: the facade closes.
        switch
            .reconcile(&BootstrapSnapshot::Unconfigured, None)
            .expect("reconcile closed");
        assert!(store.store().is_none());
    }
}
