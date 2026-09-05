use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard};

use crate::adapters::sqlite::SqliteCatalogStore;
use crate::core::bootstrap::{BootstrapService, BootstrapSnapshot, CatalogAccess};
use crate::core::domain::{
    CatalogFilter, Health, SkillDetail, SkillId, SkillSummary, parse_skill_metadata,
};
use crate::core::home::BoundHome;
use crate::core::write_gate::WriteGate;
use crate::seams::activation_store::{
    ActivationCellRow, ActivationCellWrite, ActivationObservation, ActivationStore,
    ActivationStoreError, DesiredActivation, StoredActivationObservation,
};
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedSkillRecord, LibraryConflict as AdoptConflict,
    RemoteAdoptedSkillRecord,
};
use crate::seams::agent_configuration_store::{
    AgentConfigurationStore, AgentConfigurationStoreChange, AgentConfigurationStoreError,
    AgentConfigurationStoreSnapshot, RecentProjectFolder,
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
use crate::seams::source_promotion_store::{
    LegacySourcePromotionRecord, SourcePromotionRecord, SourcePromotionStore,
    SourcePromotionStoreError,
};
use crate::seams::source_transition_store::{
    SourceTransitionRecord, SourceTransitionStore, SourceTransitionStoreError,
};
use crate::seams::source_update_store::{
    LocalSourceCopyRecord, SourceRemoveFacts, SourceUpdateRecord, SourceUpdateStore,
    SourceUpdateStoreError,
};

pub struct RuntimeCatalogStore {
    /// The current SQLite store; `None` means the catalog is closed for
    /// this bootstrap state. Swappable so Reconnect / Restore / Abandon can
    /// reopen or close the same facade every service already holds.
    sqlite: RwLock<Option<Arc<SqliteCatalogStore>>>,
    filesystem: Arc<dyn FileSystem>,
}

struct StoreReadGuard<'a> {
    slot: RwLockReadGuard<'a, Option<Arc<SqliteCatalogStore>>>,
}

impl Deref for StoreReadGuard<'_> {
    type Target = SqliteCatalogStore;

    fn deref(&self) -> &Self::Target {
        self.slot
            .as_ref()
            .expect("StoreReadGuard is created only for an available store")
            .as_ref()
    }
}

impl AsRef<SqliteCatalogStore> for StoreReadGuard<'_> {
    fn as_ref(&self) -> &SqliteCatalogStore {
        self
    }
}

/// Aligns the shared store facade with a fresh bootstrap snapshot: a Bound
/// ReadWrite result reopens the Catalog writable, a Bound read-only result
/// reopens it read-only, every other state closes the facade. Reconnect,
/// Restore commit and Abandon all route through this switch, so product
/// writes reopen or close without rebuilding the service graph.
pub struct RuntimeStoreSwitch {
    store: Arc<RuntimeCatalogStore>,
    catalog_file_name: String,
    write_gate: Arc<WriteGate>,
}

impl RuntimeStoreSwitch {
    pub fn new(
        store: Arc<RuntimeCatalogStore>,
        catalog_file_name: String,
        write_gate: Arc<WriteGate>,
    ) -> Self {
        Self {
            store,
            catalog_file_name,
            write_gate,
        }
    }

    fn reconcile(
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
            (BootstrapSnapshot::HomeUnavailable { path, .. }, _) => match self
                .store
                .reopen_read_only(&path.join(&self.catalog_file_name))
            {
                Ok(()) => Ok(()),
                Err(_error) if self.store.store().is_some() => {
                    // A disconnected volume can leave an already-open
                    // read-only handle usable. Keep that same-Home handle
                    // rather than turning a browseable HomeUnavailable
                    // session into an empty Catalog.
                    Ok(())
                }
                Err(error) => Err(error),
            },
            _ => self.store.replace_store(None),
        }
    }

    /// Align the facade after a lifecycle transition (Reconnect success,
    /// Restore commit): a fresh `inspect` clears a stale writable-open
    /// failure and recomputes the true catalog access, then the facade is
    /// reopened writable, read-only or closed to match. A durable Bound
    /// transition remains authoritative when reopening fails: close any old
    /// facade, publish CatalogReadOnly/OpenFailed through Bootstrap, then
    /// make one best-effort read-only open.
    pub fn reconcile_after_transition(
        &self,
        bootstrap: &BootstrapService,
        snapshot: &BootstrapSnapshot,
    ) -> Result<(), String> {
        let transition = self.begin_reconciliation(bootstrap, snapshot)?;
        let result = self.reconcile_after(bootstrap, snapshot, false);
        transition.commit();
        result
    }

    /// Reconcile after Existing Home Recovery. The durable transition has
    /// only recreated its app-level locator, so the first runtime open must
    /// never choose a journal mode or migrate the recovered Catalog.
    pub fn reconcile_after_existing_home_recovery(
        &self,
        bootstrap: &BootstrapService,
        snapshot: &BootstrapSnapshot,
    ) -> Result<(), String> {
        let transition = self.begin_reconciliation(bootstrap, snapshot)?;
        let result = self.reconcile_after(bootstrap, snapshot, true);
        transition.commit();
        result
    }

    fn begin_reconciliation(
        &self,
        bootstrap: &BootstrapService,
        expected: &BootstrapSnapshot,
    ) -> Result<crate::core::write_gate::HomeTransitionGuard<'_>, String> {
        let transition = self
            .write_gate
            .begin_exclusive_home_write()
            .map_err(|error| format!("cannot reconcile the runtime Catalog: {error}"))?;
        if bootstrap.inspect() != *expected {
            drop(transition);
            return Err(
                "the Home changed before the runtime Catalog reconciliation could commit".into(),
            );
        }
        Ok(transition)
    }

    fn reconcile_after(
        &self,
        bootstrap: &BootstrapService,
        snapshot: &BootstrapSnapshot,
        preserve_recovered_catalog: bool,
    ) -> Result<(), String> {
        if let BootstrapSnapshot::Bound { .. } = snapshot {
            bootstrap.clear_catalog_open_failure();
        }
        let fresh = bootstrap.inspect();
        let bound_home = bootstrap.verified_bound_home();
        let reconciled = if preserve_recovered_catalog {
            self.reconcile_recovered_existing_home(&fresh, bound_home.as_ref())
        } else {
            self.reconcile(&fresh, bound_home.as_ref())
        };
        if let Err(error) = reconciled {
            // Never retain a facade from a previous Home after its successor
            // was durably bound. The bootstrap snapshot records the failure
            // so BootstrapApi can publish the new Bound, read-only route.
            let _ = self.store.replace_store(None);
            bootstrap.note_catalog_open_failure(error);

            // A read-only open can still succeed after a writable one fails.
            // If it cannot, leave the facade closed rather than exposing
            // stale data; the OpenFailed snapshot remains authoritative.
            let degraded = bootstrap.inspect();
            let bound_home = bootstrap.verified_bound_home();
            if self.reconcile(&degraded, bound_home.as_ref()).is_err() {
                let _ = self.store.replace_store(None);
            }
        }
        Ok(())
    }

    fn reconcile_recovered_existing_home(
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
            ) => self.store.reopen_bound_without_catalog_mutation(
                home,
                &home.path.join(&self.catalog_file_name),
            ),
            _ => self.reconcile(snapshot, bound_home),
        }
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
    fn store(&self) -> Option<Arc<SqliteCatalogStore>> {
        self.sqlite.read().ok().and_then(|slot| slot.clone())
    }

    /// Swap the store (Reconnect / Restore success, Abandon close).
    fn replace_store(&self, sqlite: Option<Arc<SqliteCatalogStore>>) -> Result<(), String> {
        match self.sqlite.write() {
            Ok(mut slot) => {
                *slot = sqlite;
                Ok(())
            }
            Err(poisoned) => {
                // A poisoned facade must fail closed rather than retain an
                // old Home's store after a transition.
                *poisoned.into_inner() = None;
                Err("Runtime Catalog store lock is poisoned; facade closed".into())
            }
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
        self.replace_store(Some(Arc::new(sqlite)))
    }

    /// Reopen only a current-schema verified Catalog without persisting
    /// SQLite configuration or migrations. This is exclusive to Existing
    /// Home Recovery, whose commit contract permits no Home content writes.
    pub fn reopen_bound_without_catalog_mutation(
        &self,
        home: &BoundHome,
        catalog_path: &std::path::Path,
    ) -> Result<(), String> {
        let sqlite = SqliteCatalogStore::open_bound_without_catalog_mutation(home, catalog_path)
            .map_err(|error| error.to_string())?;
        self.replace_store(Some(Arc::new(sqlite)))
    }

    /// Reopen a Catalog read-only (a Bound Home whose writable open must
    /// not happen, e.g. integrity-failed content awaiting Restore).
    pub fn reopen_read_only(&self, catalog_path: &std::path::Path) -> Result<(), String> {
        let sqlite =
            SqliteCatalogStore::open_read_only(catalog_path).map_err(|error| error.to_string())?;
        self.replace_store(Some(Arc::new(sqlite)))
    }

    fn writable_store(&self) -> Option<StoreReadGuard<'_>> {
        let slot = self.sqlite.read().ok()?;
        let sqlite = slot.as_ref()?;
        if sqlite.startup_status().access != StartupAccess::ReadWrite {
            return None;
        }
        Some(StoreReadGuard { slot })
    }

    fn writable_store_or<E, F>(&self, unavailable: F) -> Result<StoreReadGuard<'_>, E>
    where
        F: FnOnce() -> E,
    {
        self.writable_store().ok_or_else(unavailable)
    }

    #[cfg(test)]
    fn is_writable(&self) -> bool {
        self.writable_store().is_some()
    }

    /// The current store or the closed `CatalogStore` error — the fail-closed
    /// path for read commands outside `Bound`.
    fn require_catalog(&self) -> Result<StoreReadGuard<'_>, CatalogStoreError> {
        let slot = self.sqlite.read().map_err(|_| {
            CatalogStoreError::Unavailable("the catalog store lock is poisoned".into())
        })?;
        if slot.is_none() {
            return Err(CatalogStoreError::Unavailable(
                "no Bound Home: the catalog is closed".into(),
            ));
        }
        Ok(StoreReadGuard { slot })
    }
}

impl CatalogStore for RuntimeCatalogStore {
    fn snapshot_version(&self) -> u64 {
        let Ok(slot) = self.sqlite.read() else {
            return 0;
        };
        slot.as_ref()
            .and_then(|sqlite| sqlite.persisted_snapshot_version().ok())
            .unwrap_or(0)
    }

    fn list(&self, filter: CatalogFilter) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        self.require_catalog()?.list_skill_summaries(filter)
    }

    fn skill_directory_identity_key(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<String>, CatalogStoreError> {
        self.require_catalog()?
            .skill_directory_identity_key(skill_id)
    }

    fn inspect(&self, skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError> {
        let catalog = self.require_catalog()?;
        let Some(persisted) = catalog.persisted_skill_detail(skill_id)? else {
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

    fn first_run_completed_at(&self) -> Result<Option<String>, CatalogStoreError> {
        self.require_catalog()?.first_run_completed_at()
    }

    fn mark_first_run_completed(&self) -> Result<(), CatalogStoreError> {
        let sqlite = self.writable_store_or(|| {
            CatalogStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.mark_first_run_completed()
    }

    fn recently_enabled(&self, limit: u32) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        let Some(sqlite) = self.writable_store() else {
            return Ok(Vec::new());
        };
        sqlite.recently_enabled_skills(limit)
    }
}

impl SourcePromotionStore for RuntimeCatalogStore {
    fn read_legacy_source_promotion(
        &self,
        remote_id: &str,
    ) -> Result<LegacySourcePromotionRecord, SourcePromotionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourcePromotionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.read_legacy_source_promotion(remote_id)
    }

    fn validate_source_promotion(
        &self,
        record: &SourcePromotionRecord,
    ) -> Result<(), SourcePromotionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourcePromotionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.validate_source_promotion(record)
    }

    fn commit_source_promotion(
        &self,
        record: SourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourcePromotionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.commit_source_promotion(record)
    }

    fn source_promotion_is_committed(
        &self,
        record: &SourcePromotionRecord,
    ) -> Result<bool, SourcePromotionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourcePromotionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.source_promotion_is_committed(record)
    }

    fn undo_source_promotion(
        &self,
        record: &SourcePromotionRecord,
        legacy: &LegacySourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourcePromotionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.undo_source_promotion(record, legacy)
    }
}

impl AgentConfigurationStore for RuntimeCatalogStore {
    fn agent_configuration_snapshot(
        &self,
    ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError> {
        let sqlite = self
            .require_catalog()
            .map_err(|error| AgentConfigurationStoreError::Unavailable(error.to_string()))?;
        sqlite.agent_configuration_snapshot()
    }

    fn apply_agent_configuration_change(
        &self,
        expected_snapshot_version: u64,
        change: AgentConfigurationStoreChange,
    ) -> Result<u64, AgentConfigurationStoreError> {
        let sqlite = self.writable_store_or(|| {
            AgentConfigurationStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.apply_agent_configuration_change(expected_snapshot_version, change)
    }

    fn list_recent_project_folders(
        &self,
    ) -> Result<Vec<RecentProjectFolder>, AgentConfigurationStoreError> {
        let sqlite = self
            .require_catalog()
            .map_err(|error| AgentConfigurationStoreError::Unavailable(error.to_string()))?;
        sqlite.list_recent_project_folders()
    }

    fn record_recent_project_folder(
        &self,
        folder: RecentProjectFolder,
    ) -> Result<(), AgentConfigurationStoreError> {
        let sqlite = self.writable_store_or(|| {
            AgentConfigurationStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.record_recent_project_folder(folder)
    }

    fn clear_recent_project_folders(&self) -> Result<(), AgentConfigurationStoreError> {
        let sqlite = self.writable_store_or(|| {
            AgentConfigurationStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.clear_recent_project_folders()
    }
}

impl SourceUpdateStore for RuntimeCatalogStore {
    fn read_current(
        &self,
        remote_id: &str,
    ) -> Result<
        Option<crate::seams::source_update_store::SourceUpdateCurrentSource>,
        SourceUpdateStoreError,
    > {
        self.require_catalog()
            .map_err(|error| SourceUpdateStoreError::Unavailable(error.to_string()))?
            .read_current(remote_id)
    }

    fn source_ids(&self) -> Result<Vec<String>, SourceUpdateStoreError> {
        self.require_catalog()
            .map_err(|error| SourceUpdateStoreError::Unavailable(error.to_string()))?
            .source_ids()
    }

    fn member_health(
        &self,
        skill_id: &crate::core::domain::SkillId,
    ) -> Result<Option<(String, crate::core::domain::Health)>, SourceUpdateStoreError> {
        self.require_catalog()
            .map_err(|error| SourceUpdateStoreError::Unavailable(error.to_string()))?
            .member_health(skill_id)
    }

    fn validate_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<(), SourceUpdateStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceUpdateStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.validate_source_update(record)
    }

    fn commit_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceUpdateStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.commit_source_update(record)
    }

    fn source_update_is_committed(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<bool, SourceUpdateStoreError> {
        self.require_catalog()
            .map_err(|error| SourceUpdateStoreError::Unavailable(error.to_string()))?
            .source_update_is_committed(record)
    }

    fn undo_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceUpdateStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.undo_source_update(record)
    }

    fn set_source_member_health(
        &self,
        remote_id: &str,
        health: &[(crate::core::domain::SkillId, crate::core::domain::Health)],
    ) -> Result<u64, SourceUpdateStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceUpdateStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.set_source_member_health(remote_id, health)
    }

    fn register_local_copy(
        &self,
        record: &LocalSourceCopyRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceUpdateStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.register_local_copy(record)
    }

    fn local_copy_is_registered(&self, destination: &Path) -> Result<bool, SourceUpdateStoreError> {
        self.require_catalog()
            .map_err(|error| SourceUpdateStoreError::Unavailable(error.to_string()))?
            .local_copy_is_registered(destination)
    }

    fn source_remove_facts(
        &self,
        remote_id: &str,
    ) -> Result<SourceRemoveFacts, SourceUpdateStoreError> {
        self.require_catalog()
            .map_err(|error| SourceUpdateStoreError::Unavailable(error.to_string()))?
            .source_remove_facts(remote_id)
    }

    fn commit_remove_source(&self, remote_id: &str) -> Result<u64, SourceUpdateStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceUpdateStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.commit_remove_source(remote_id)
    }

    fn source_remove_is_committed(&self, remote_id: &str) -> Result<bool, SourceUpdateStoreError> {
        self.require_catalog()
            .map_err(|error| SourceUpdateStoreError::Unavailable(error.to_string()))?
            .source_remove_is_committed(remote_id)
    }
}

impl SourceTransitionStore for RuntimeCatalogStore {
    fn existing_current_members(
        &self,
        canonical_url: &str,
    ) -> Result<
        Option<Vec<crate::seams::source_transition_store::ExistingSourceMember>>,
        SourceTransitionStoreError,
    > {
        let sqlite = self.writable_store_or(|| {
            SourceTransitionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.existing_current_members(canonical_url)
    }

    fn existing_source(
        &self,
        remote_id: &str,
    ) -> Result<
        Option<crate::seams::source_transition_store::ExistingSourceFacts>,
        SourceTransitionStoreError,
    > {
        let sqlite = self.writable_store_or(|| {
            SourceTransitionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.existing_source(remote_id)
    }

    fn validate_new_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<(), SourceTransitionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceTransitionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.validate_new_source_transition(record)
    }

    fn commit_source_transition(
        &self,
        record: SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceTransitionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.commit_source_transition(record)
    }

    fn source_transition_is_committed(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<bool, SourceTransitionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceTransitionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.source_transition_is_committed(record)
    }

    fn undo_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        let sqlite = self.writable_store_or(|| {
            SourceTransitionStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.undo_source_transition(record)
    }
}

impl ImportStore for RuntimeCatalogStore {
    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<LibraryConflict>, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        crate::seams::import_store::ImportStore::find_library_conflict(
            sqlite.as_ref(),
            identity_key,
        )
    }

    fn insert_link(&self, record: LinkImportRecord) -> Result<u64, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.insert_link(record)
    }

    fn insert_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.insert_file(record)
    }

    fn insert_files(&self, records: Vec<FileImportRecord>) -> Result<u64, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.insert_files(records)
    }

    fn load_file_install(
        &self,
        identity_key: &str,
    ) -> Result<Option<FileImportRecord>, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.load_file_install(identity_key)
    }

    fn desired_activations_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<DesiredActivation>, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.desired_activations_for_skill(skill_id)
    }

    fn replace_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.replace_file(record)
    }

    fn insert_remotes(&self, records: Vec<RemoteImportRecord>) -> Result<u64, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.insert_remotes(records)
    }

    fn load_remote_installs(&self) -> Result<Vec<RemoteInstallRecord>, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.load_remote_installs()
    }

    fn load_remote_install(
        &self,
        identity_key: &str,
    ) -> Result<Option<RemoteInstallRecord>, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.load_remote_install(identity_key)
    }

    fn update_remote_install(&self, record: RemoteImportRecord) -> Result<u64, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.update_remote_install(record)
    }

    fn record_remote_check(&self, skill_id: &SkillId) -> Result<(), ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.record_remote_check(skill_id)
    }

    fn set_remote_requested_ref(
        &self,
        skill_id: &SkillId,
        expected_requested_ref: &str,
        expected_verification_anchor: &str,
        requested_ref: &str,
    ) -> Result<(), ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.set_remote_requested_ref(
            skill_id,
            expected_requested_ref,
            expected_verification_anchor,
            requested_ref,
        )
    }

    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        crate::seams::import_store::ImportStore::find_remote_parent_by_url(
            sqlite.as_ref(),
            canonical_url,
        )
    }

    fn load_remote_parents(&self) -> Result<Vec<RemoteParentRecord>, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.load_remote_parents()
    }

    fn insert_remote_alias(
        &self,
        remote_id: &str,
        alias_url: &str,
    ) -> Result<(), ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.insert_remote_alias(remote_id, alias_url)
    }

    fn delete_remote_parent_if_last_child(
        &self,
        remote_id: &str,
    ) -> Result<bool, ImportStoreError> {
        let sqlite = self.writable_store_or(|| {
            ImportStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        crate::seams::import_store::ImportStore::delete_remote_parent_if_last_child(
            sqlite.as_ref(),
            remote_id,
        )
    }
}

impl AdoptStore for RuntimeCatalogStore {
    fn snapshot_version(&self) -> u64 {
        <Self as crate::seams::catalog_store::CatalogStore>::snapshot_version(self)
    }

    fn list_agents(&self) -> Result<Vec<AdoptAgent>, AdoptStoreError> {
        let sqlite = self.writable_store_or(|| {
            AdoptStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.list_agents()
    }

    fn insert_adopted(&self, record: AdoptedSkillRecord) -> Result<u64, AdoptStoreError> {
        let sqlite = self.writable_store_or(|| {
            AdoptStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.insert_adopted(record)
    }

    fn remove_adopted_skill(&self, skill_id: &SkillId) -> Result<u64, AdoptStoreError> {
        let sqlite = self.writable_store_or(|| {
            AdoptStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.remove_adopted_skill(skill_id)
    }

    fn insert_remote_adopted(
        &self,
        record: RemoteAdoptedSkillRecord,
    ) -> Result<u64, AdoptStoreError> {
        let sqlite = self.writable_store_or(|| {
            AdoptStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.insert_remote_adopted(record)
    }

    fn delete_remote_parent_if_last_child(&self, remote_id: &str) -> Result<bool, AdoptStoreError> {
        let sqlite = self.writable_store_or(|| {
            AdoptStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        crate::seams::adopt_store::AdoptStore::delete_remote_parent_if_last_child(
            sqlite.as_ref(),
            remote_id,
        )
    }

    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, AdoptStoreError> {
        let sqlite = self.writable_store_or(|| {
            AdoptStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        crate::seams::adopt_store::AdoptStore::find_remote_parent_by_url(
            sqlite.as_ref(),
            canonical_url,
        )
    }

    fn binding_remote_id(&self, skill_id: &SkillId) -> Result<Option<String>, AdoptStoreError> {
        let sqlite = self.writable_store_or(|| {
            AdoptStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        crate::seams::adopt_store::AdoptStore::binding_remote_id(sqlite.as_ref(), skill_id)
    }

    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<AdoptConflict>, AdoptStoreError> {
        let sqlite = self.writable_store_or(|| {
            AdoptStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        crate::seams::adopt_store::AdoptStore::find_library_conflict(sqlite.as_ref(), identity_key)
    }
}

impl ActivationStore for RuntimeCatalogStore {
    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        let sqlite = self
            .require_catalog()
            .map_err(|error| ActivationStoreError::Unavailable(error.to_string()))?;
        sqlite.desired_activations()
    }

    fn activation_observations(
        &self,
    ) -> Result<Vec<StoredActivationObservation>, ActivationStoreError> {
        let sqlite = self
            .require_catalog()
            .map_err(|error| ActivationStoreError::Unavailable(error.to_string()))?;
        sqlite.activation_observations()
    }

    fn record_observations(
        &self,
        observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError> {
        let sqlite = self.writable_store_or(|| {
            ActivationStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.record_observations(observations)
    }

    fn record_observation(
        &self,
        observation: &ActivationObservation,
    ) -> Result<u64, ActivationStoreError> {
        let sqlite = self.writable_store_or(|| {
            ActivationStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.record_observation(observation)
    }

    fn activation_cells(&self) -> Result<Vec<ActivationCellRow>, ActivationStoreError> {
        let sqlite = self
            .require_catalog()
            .map_err(|error| ActivationStoreError::Unavailable(error.to_string()))?;
        sqlite.activation_cells()
    }

    fn activation_cells_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<ActivationCellRow>, ActivationStoreError> {
        let sqlite = self
            .require_catalog()
            .map_err(|error| ActivationStoreError::Unavailable(error.to_string()))?;
        sqlite.activation_cells_for_skill(skill_id)
    }

    fn write_activation_cells(
        &self,
        writes: &[ActivationCellWrite],
    ) -> Result<u64, ActivationStoreError> {
        let sqlite = self.writable_store_or(|| {
            ActivationStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.write_activation_cells(writes)
    }

    fn catalog_generation(&self) -> Result<u64, ActivationStoreError> {
        let sqlite = self
            .require_catalog()
            .map_err(|error| ActivationStoreError::Unavailable(error.to_string()))?;
        sqlite.catalog_generation()
    }
}

impl MaintenanceStore for RuntimeCatalogStore {
    fn adopted_skill_entities(&self) -> Result<Vec<AdoptedSkillEntity>, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; Adopt recovery entities are unavailable".into(),
            )
        })?;
        sqlite.adopted_skill_entities()
    }

    fn installed_skill_baselines(
        &self,
    ) -> Result<Vec<InstalledSkillBaseline>, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; recovery baselines are unavailable".into(),
            )
        })?;
        sqlite.installed_skill_baselines()
    }

    fn record_skill_health(
        &self,
        observations: &[SkillHealthObservation],
    ) -> Result<u64, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.record_skill_health(observations)
    }

    fn managed_skill_baselines(&self) -> Result<Vec<ManagedSkillBaseline>, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable(
                "catalog startup is read-only; health baselines are unavailable".into(),
            )
        })?;
        sqlite.managed_skill_baselines()
    }

    fn link_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<LinkSkillRecord>, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.link_skill(skill_id)
    }

    fn activation_baselines_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<RelocateActivationBaseline>, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.activation_baselines_for_skill(skill_id)
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
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.commit_relocate(
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
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.remove_target(skill_id)
    }

    fn is_git_source_member(&self, skill_id: &SkillId) -> Result<bool, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.is_git_source_member(skill_id)
    }

    fn delete_skill(&self, skill_id: &SkillId) -> Result<u64, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.delete_skill(skill_id)
    }

    fn binding_remote_id(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<String>, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.binding_remote_id(skill_id)
    }

    fn delete_remote_parent_if_last_child(
        &self,
        remote_id: &str,
    ) -> Result<bool, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.delete_remote_parent_if_last_child(remote_id)
    }

    fn insert_handoff_recovered(
        &self,
        record: HandoffRecoveredRecord,
    ) -> Result<u64, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        sqlite.insert_handoff_recovered(record)
    }

    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable("catalog startup is read-only".into())
        })?;
        crate::seams::import_store::ImportStore::find_remote_parent_by_url(
            sqlite.as_ref(),
            canonical_url,
        )
        .map_err(|error| MaintenanceStoreError::Unavailable(error.to_string()))
    }

    fn desired_activation_baselines(
        &self,
    ) -> Result<Vec<ActivationRecoveryBaseline>, MaintenanceStoreError> {
        let sqlite = self.writable_store_or(|| {
            MaintenanceStoreError::Unavailable(
            "catalog startup is read-only; Activation replace recovery baselines are unavailable"
                .into(),
        )
        })?;
        crate::seams::activation_store::ActivationStore::desired_activations(sqlite.as_ref())
            .map(|activations| {
                activations
                    .into_iter()
                    .map(|activation| ActivationRecoveryBaseline {
                        skill_id: activation.skill_id.0,
                        target_root_id: activation.target_root_id,
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
        let sqlite = self.require_catalog().map_err(|error| {
            crate::seams::preferences_store::PreferencesStoreError::Unavailable(error.to_string())
        })?;
        sqlite.load_preferences()
    }

    fn update_preferences(
        &self,
        updates: crate::seams::preferences_store::PreferenceUpdates,
    ) -> Result<
        crate::seams::preferences_store::AppPreferences,
        crate::seams::preferences_store::PreferencesStoreError,
    > {
        let sqlite = self.writable_store_or(|| {
            crate::seams::preferences_store::PreferencesStoreError::Unavailable(
                "catalog startup is read-only".into(),
            )
        })?;
        sqlite.update_preferences(updates)
    }

    fn last_app_update_check_at(
        &self,
    ) -> Result<Option<i64>, crate::seams::preferences_store::PreferencesStoreError> {
        let sqlite = self.require_catalog().map_err(|error| {
            crate::seams::preferences_store::PreferencesStoreError::Unavailable(error.to_string())
        })?;
        sqlite.last_app_update_check_at()
    }

    fn record_app_update_check_at(
        &self,
        checked_at: i64,
    ) -> Result<(), crate::seams::preferences_store::PreferencesStoreError> {
        let sqlite = self.writable_store_or(|| {
            crate::seams::preferences_store::PreferencesStoreError::Unavailable(
                "catalog startup is read-only".into(),
            )
        })?;
        sqlite.record_app_update_check_at(checked_at)
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
        assert_eq!(
            <RuntimeCatalogStore as crate::seams::catalog_store::CatalogStore>::snapshot_version(
                &store
            ),
            0
        );
        assert!(matches!(
            store.list(CatalogFilter::All),
            Err(CatalogStoreError::Unavailable(_))
        ));
        assert!(matches!(
            <RuntimeCatalogStore as crate::seams::agent_configuration_store::AgentConfigurationStore>::agent_configuration_snapshot(
                &store
            ),
            Err(AgentConfigurationStoreError::Unavailable(_))
        ));
        assert!(matches!(
            <RuntimeCatalogStore as crate::seams::activation_store::ActivationStore>::catalog_generation(
                &store
            ),
            Err(ActivationStoreError::Unavailable(_))
        ));
        assert!(matches!(
            <RuntimeCatalogStore as crate::seams::preferences_store::PreferencesStore>::load_preferences(
                &store
            ),
            Err(crate::seams::preferences_store::PreferencesStoreError::Unavailable(_))
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
        store.replace_store(None).expect("close facade");
        assert!(store.store().is_none());
        assert!(matches!(
            store.list(CatalogFilter::All),
            Err(CatalogStoreError::Unavailable(_))
        ));
    }

    #[test]
    fn an_in_flight_catalog_read_serializes_with_store_replacement() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (store, _home) = open_store(dir.path());
        let reader_store = store.clone();

        // The facade holds its read lock through the delegated operation, so
        // replacement waits for an in-flight call instead of exposing an old
        // Home after the swap.
        let reader = std::thread::spawn(move || reader_store.first_run_completed_at());
        store.replace_store(None).expect("close facade");
        assert!(matches!(
            reader.join().expect("Catalog reader"),
            Ok(_) | Err(CatalogStoreError::Unavailable(_))
        ));
        assert!(matches!(
            store.first_run_completed_at(),
            Err(CatalogStoreError::Unavailable(_))
        ));
        assert!(store.store().is_none());
    }

    #[test]
    fn store_switch_reconciles_snapshot_to_open_or_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (store, home) = open_store(dir.path());
        let switch = RuntimeStoreSwitch::new(
            store.clone(),
            "skill-man.sqlite3".into(),
            Arc::new(WriteGate::open_for_tests()),
        );

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

    #[test]
    fn home_unavailable_preserves_an_existing_catalog_handle_when_reopen_fails() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (store, home) = open_store(dir.path());
        let switch = RuntimeStoreSwitch::new(
            store.clone(),
            "skill-man.sqlite3".into(),
            Arc::new(WriteGate::open_for_tests()),
        );
        let missing_home = home.path.join("disconnected");
        switch
            .reconcile(
                &BootstrapSnapshot::HomeUnavailable {
                    home_id: home.home_id,
                    path: missing_home,
                    diagnostic: None,
                },
                None,
            )
            .expect("existing read-only handle remains usable");
        assert!(store.store().is_some());
    }
}
