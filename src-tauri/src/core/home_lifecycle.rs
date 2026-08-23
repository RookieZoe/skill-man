//! Home Lifecycle core (spec §5.5, §4.2; ADR-0012 §5–§6): Reconnect Same
//! Home and Abandon Home and Start New — the only identity-related actions
//! after a binding exists (no Preferences re-path, no Relocate, no ordinary
//! re-home).
//!
//! - Reconnect re-runs the three-way verification through the bootstrap
//!   authority and restores the same `home_id` only when volume, marker and
//!   Catalog agree again. It never writes the locator, the Home or the
//!   ledger; a failed reconnect keeps the closed state with its diagnostic.
//! - Abandon is the high-friction escape hatch: the typed confirmation must
//!   match the binding's `home_id`, and the locator CAS that moves
//!   `current` permanently into the abandoned history is the single commit
//!   point. The old Home, its Activations and the locale are never touched;
//!   the next binding gets a brand-new UUID.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use thiserror::Error;

use crate::core::bootstrap::{BootstrapService, BootstrapSnapshot};
use crate::core::home::HomeId;
use crate::seams::app_state_store::{AbandonedHomeRecord, AppStateStore, AppStateStoreError};

/// The high-friction preview a user confirms before Abandon (ADR-0012 §6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbandonPreview {
    pub home_id: HomeId,
    pub path: PathBuf,
    pub bound_at: String,
    pub plan_token: String,
}

#[derive(Debug, Error)]
pub enum HomeLifecycleError {
    #[error("Reconnect is only available from HomeUnavailable or HomeIdentityMismatch")]
    ReconnectNotAvailable,
    #[error("Abandon requires an active Home Binding; current bootstrap state: {0}")]
    NotAbandonable(String),
    #[error("Abandon is blocked by an active operation: {operation_id}")]
    ActiveOperation { operation_id: String },
    #[error("the Abandon confirmation does not match the binding home_id")]
    ConfirmationMismatch,
    #[error("the bootstrap locator changed before the Abandon could be committed: {0}")]
    CasConflict(String),
    #[error("the Abandon plan is stale; review it again")]
    PlanStale,
    #[error("the app state could not be read or written: {0}")]
    StateStore(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<AppStateStoreError> for HomeLifecycleError {
    fn from(error: AppStateStoreError) -> Self {
        match error {
            AppStateStoreError::LocatorCasConflict { .. } => {
                HomeLifecycleError::CasConflict(error.to_string())
            }
            other => HomeLifecycleError::StateStore(other.to_string()),
        }
    }
}

#[derive(Clone)]
struct AbandonPlan {
    home_id: HomeId,
    path: PathBuf,
    volume_fsid: String,
    volume_uuid: String,
}

pub struct HomeLifecycleService {
    app_state: Arc<dyn AppStateStore>,
    bootstrap: Arc<BootstrapService>,
    plans: RwLock<HashMap<String, AbandonPlan>>,
}

impl HomeLifecycleService {
    pub fn new(app_state: Arc<dyn AppStateStore>, bootstrap: Arc<BootstrapService>) -> Self {
        Self {
            app_state,
            bootstrap,
            plans: RwLock::new(HashMap::new()),
        }
    }

    /// Reconnect Same Home (spec §5.5): user-initiated re-verification of
    /// the existing binding (the UI exposes the action only from
    /// HomeUnavailable / HomeIdentityMismatch). The bootstrap authority
    /// re-runs volume → marker → Catalog → identity; only a
    /// four-way-verified `Bound` result restores the same `home_id`.
    /// Otherwise the closed state returns unchanged — no locator, path or
    /// ledger side effects, only the diagnostic the snapshot already
    /// carries. Read-only; a current binding must exist.
    pub fn reconnect_same_home(&self) -> Result<BootstrapSnapshot, HomeLifecycleError> {
        let files = self.app_state.load()?;
        if files.binding.current.is_none() {
            return Err(HomeLifecycleError::ReconnectNotAvailable);
        }
        Ok(self.bootstrap.inspect())
    }

    /// Abandon Home and Start New — preview step (ADR-0012 §6). Eligible in
    /// every bound state (Bound, HomeUnavailable, HomeIdentityMismatch and
    /// the bound Fixture Recovery Lock); never in Legacy Lock (no binding),
    /// which would constitute a bypass. An active recovery/binding
    /// operation blocks Abandon so a half-done operation is never abandoned
    /// under the app; the operation must be continued or cancelled first.
    /// Read-only: no locator, Home or ledger mutation.
    pub fn plan_abandon(&self) -> Result<AbandonPreview, HomeLifecycleError> {
        let files = self.app_state.load()?;
        if let Some(active) = &files.recovery_ledger.active {
            return Err(HomeLifecycleError::ActiveOperation {
                operation_id: active.operation_id.clone(),
            });
        }
        match self.bootstrap.inspect() {
            BootstrapSnapshot::Bound { .. }
            | BootstrapSnapshot::HomeUnavailable { .. }
            | BootstrapSnapshot::HomeIdentityMismatch { .. }
            | BootstrapSnapshot::FixtureRecoveryLocked {
                home_id: Some(_), ..
            } => {}
            other => {
                return Err(HomeLifecycleError::NotAbandonable(format!("{other:?}")));
            }
        }
        let current = files
            .binding
            .current
            .ok_or_else(|| HomeLifecycleError::NotAbandonable("no current binding".into()))?;
        let plan_token = format!("ab-{:x}", token_nanos());
        let plan = AbandonPlan {
            home_id: current.home_id.clone(),
            path: current.path.clone(),
            volume_fsid: current.volume_fsid.clone(),
            volume_uuid: current.volume_uuid.clone(),
        };
        self.plans
            .write()
            .map_err(|_| HomeLifecycleError::Internal("plan table poisoned".into()))?
            .insert(plan_token.clone(), plan);
        Ok(AbandonPreview {
            home_id: current.home_id,
            path: current.path,
            bound_at: current.bound_at,
            plan_token,
        })
    }

    /// Apply an Abandon plan: the locator CAS is the only commit point. The
    /// typed confirmation must exactly match the binding's `home_id`, the
    /// plan must be current, and the CAS must still see the same `current`
    /// binding — a concurrent binding commit or a second Abandon surfaces
    /// `CasConflict` and nothing is written. After the CAS the old Home,
    /// its Activations and the locale are untouched; the fresh snapshot is
    /// `Unconfigured`, or the `Abandoned` notice route when the abandoned
    /// site sits at the default path.
    pub fn apply_abandon(
        &self,
        plan_token: &str,
        confirmation_home_id: &HomeId,
    ) -> Result<BootstrapSnapshot, HomeLifecycleError> {
        let plan = self
            .plans
            .read()
            .map_err(|_| HomeLifecycleError::Internal("plan table poisoned".into()))?
            .get(plan_token)
            .cloned()
            .ok_or(HomeLifecycleError::PlanStale)?;
        if plan.home_id != *confirmation_home_id {
            return Err(HomeLifecycleError::ConfirmationMismatch);
        }
        let files = self.app_state.load()?;
        if let Some(active) = &files.recovery_ledger.active {
            return Err(HomeLifecycleError::ActiveOperation {
                operation_id: active.operation_id.clone(),
            });
        }
        // The CAS is the single authority: a missing, replaced or
        // concurrently-committed current binding all fail the same compare
        // and nothing is written.
        let mut next = files.binding.clone();
        next.current = None;
        next.abandoned.push(AbandonedHomeRecord {
            home_id: plan.home_id.clone(),
            path: plan.path.clone(),
            volume_fsid: plan.volume_fsid.clone(),
            volume_uuid: plan.volume_uuid.clone(),
            abandoned_at: rfc3339_now(),
        });
        self.app_state
            .cas_locator(Some(&plan.home_id), &next)
            .map_err(HomeLifecycleError::from)?;
        if let Ok(mut plans) = self.plans.write() {
            plans.remove(plan_token);
        }
        Ok(self.bootstrap.inspect())
    }
}

// -- helpers ---------------------------------------------------------------

fn token_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn rfc3339_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    crate::core::fixture_recovery::epoch_seconds_to_rfc3339(seconds)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use crate::adapters::macos_fs::MacOsFileSystem;
    use crate::core::bootstrap::BootstrapConfig;
    use crate::core::fixture_recovery::{
        FixtureClassification, FixtureClassifier, FixtureShapeMode,
    };
    use crate::core::home::{HomeMarker, VolumeIdentity};
    use crate::seams::app_state_store::{
        AppStateFiles, HomeBindingFile, HomeBindingRecord, RecoveryLedgerFile,
        RecoveryOperationRecord,
    };
    use crate::seams::catalog_probe::{
        CatalogHomeIdentity, CatalogProbe, CatalogProbeError, CatalogProbeReport,
    };
    use crate::seams::filesystem::FileSystem;
    use crate::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

    use super::*;

    const HOME_ID: &str = "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";

    struct MemoryAppStateStore {
        state: Mutex<AppStateFiles>,
    }

    impl MemoryAppStateStore {
        fn new(files: AppStateFiles) -> Self {
            Self {
                state: Mutex::new(files),
            }
        }
    }

    impl AppStateStore for MemoryAppStateStore {
        fn load(&self) -> Result<AppStateFiles, AppStateStoreError> {
            Ok(self.state.lock().unwrap().clone())
        }

        fn write_locator(&self, binding: &HomeBindingFile) -> Result<(), AppStateStoreError> {
            self.state.lock().unwrap().binding = binding.clone();
            Ok(())
        }

        fn write_recovery_ledger(
            &self,
            ledger: &RecoveryLedgerFile,
        ) -> Result<(), AppStateStoreError> {
            self.state.lock().unwrap().recovery_ledger = ledger.clone();
            Ok(())
        }
    }

    struct ToggleVolumeIdentitySource {
        volume: Mutex<Option<VolumeIdentity>>,
    }

    impl ToggleVolumeIdentitySource {
        fn set(&self, volume: Option<VolumeIdentity>) {
            *self.volume.lock().unwrap() = volume;
        }
    }

    impl VolumeIdentitySource for ToggleVolumeIdentitySource {
        fn volume_identity(
            &self,
            _path: &Path,
        ) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
            Ok(self.volume.lock().unwrap().clone())
        }
    }

    struct MemoryCatalogProbe(CatalogProbeReport);

    impl CatalogProbe for MemoryCatalogProbe {
        fn probe(&self, _path: &Path) -> Result<CatalogProbeReport, CatalogProbeError> {
            Ok(self.0.clone())
        }
    }

    struct FixedClassifier(FixtureClassification);

    impl FixtureClassifier for FixedClassifier {
        fn classify(&self, _home_root: &Path, _mode: FixtureShapeMode) -> FixtureClassification {
            self.0.clone()
        }
    }

    fn bound_files(home_path: &Path, volume: &VolumeIdentity) -> AppStateFiles {
        AppStateFiles {
            binding: HomeBindingFile {
                schema_version: 1,
                current: Some(HomeBindingRecord {
                    home_id: HomeId(HOME_ID.into()),
                    path: home_path.to_path_buf(),
                    volume_fsid: volume.fsid.clone(),
                    volume_uuid: volume.uuid.clone(),
                    bound_at: "2026-08-01T00:00:00Z".into(),
                }),
                abandoned: vec![],
            },
            recovery_ledger: RecoveryLedgerFile::empty(),
        }
    }

    fn probe_report(volume: &VolumeIdentity) -> CatalogProbeReport {
        CatalogProbeReport {
            exists: true,
            schema_version: Some(5),
            integrity_ok: true,
            foreign_keys_ok: true,
            home_identity: Some(CatalogHomeIdentity {
                home_id: HomeId(HOME_ID.into()),
                volume_fsid: volume.fsid.clone(),
                volume_uuid: volume.uuid.clone(),
                home_bound_at: "2026-08-01T00:00:00Z".into(),
            }),
            snapshot_version: Some(3),
        }
    }

    fn marker(dir: &Path, home_path: &Path, volume: &VolumeIdentity) {
        let filesystem = MacOsFileSystem::new(dir.to_path_buf());
        filesystem.ensure_directory(home_path).expect("home dir");
        filesystem
            .write_utf8_file(
                &home_path.join(HomeMarker::FILE_NAME),
                &serde_json::to_string_pretty(&HomeMarker {
                    schema_version: HomeMarker::SCHEMA_VERSION,
                    home_id: HomeId(HOME_ID.into()),
                    volume_fsid: volume.fsid.clone(),
                    volume_uuid: volume.uuid.clone(),
                    created_at: "2026-08-01T00:00:00Z".into(),
                })
                .expect("marker JSON"),
            )
            .expect("write marker");
    }

    fn compose_lifecycle(
        dir: &Path,
        app_state: Arc<MemoryAppStateStore>,
        volume: Arc<ToggleVolumeIdentitySource>,
        probe: CatalogProbeReport,
        classifier: FixtureClassification,
    ) -> HomeLifecycleService {
        let bootstrap = BootstrapService::new(
            app_state.clone(),
            volume,
            Arc::new(MemoryCatalogProbe(probe)),
            Arc::new(MacOsFileSystem::new(dir.to_path_buf())),
            Arc::new(FixedClassifier(classifier)),
            BootstrapConfig {
                state_dir: dir.join("state"),
                default_home_path: dir.join("default-home"),
                catalog_file_name: "skill-man.sqlite3".into(),
            },
        );
        HomeLifecycleService::new(app_state, Arc::new(bootstrap))
    }

    fn volume() -> VolumeIdentity {
        VolumeIdentity {
            fsid: "fsid-1".into(),
            uuid: "uuid-1".into(),
        }
    }

    #[test]
    fn reconnect_restores_the_same_home_id_when_the_volume_returns() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        marker(dir.path(), &home, &volume());
        let files = bound_files(&home, &volume());

        // Volume offline: HomeUnavailable, nothing written.
        let app_state = Arc::new(MemoryAppStateStore::new(files.clone()));
        let volume_source = Arc::new(ToggleVolumeIdentitySource {
            volume: Mutex::new(None),
        });
        let lifecycle = compose_lifecycle(
            dir.path(),
            app_state.clone(),
            volume_source.clone(),
            probe_report(&volume()),
            FixtureClassification::Clean,
        );
        match lifecycle.reconnect_same_home().expect("reconnect") {
            BootstrapSnapshot::HomeUnavailable { home_id, .. } => {
                assert_eq!(home_id.0, HOME_ID);
            }
            other => panic!("expected HomeUnavailable, got {other:?}"),
        }
        assert_eq!(
            app_state.load().expect("app state").binding,
            files.binding,
            "a failed reconnect has zero locator side effects"
        );

        // Volume back with the same identity: the same service instance
        // reconnects to the same home_id.
        volume_source.set(Some(volume()));
        match lifecycle.reconnect_same_home().expect("reconnect") {
            BootstrapSnapshot::Bound { home_id, .. } => {
                assert_eq!(home_id.0, HOME_ID, "the same home_id is restored");
            }
            other => panic!("expected Bound, got {other:?}"),
        }
        assert_eq!(
            app_state.load().expect("app state").binding,
            files.binding,
            "a successful reconnect also has zero locator side effects"
        );
    }

    #[test]
    fn reconnect_failure_keeps_mismatch_without_side_effects() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let other_volume = VolumeIdentity {
            fsid: "fsid-1".into(),
            uuid: "other-uuid".into(),
        };
        marker(dir.path(), &home, &other_volume);
        let files = bound_files(&home, &volume());
        let app_state = Arc::new(MemoryAppStateStore::new(files.clone()));
        let volume_source = Arc::new(ToggleVolumeIdentitySource {
            volume: Mutex::new(Some(other_volume.clone())),
        });
        let lifecycle = compose_lifecycle(
            dir.path(),
            app_state.clone(),
            volume_source,
            probe_report(&volume()),
            FixtureClassification::Clean,
        );
        match lifecycle.reconnect_same_home().expect("reconnect") {
            BootstrapSnapshot::HomeIdentityMismatch { home_id, .. } => {
                assert_eq!(home_id.0, HOME_ID);
            }
            other => panic!("expected HomeIdentityMismatch, got {other:?}"),
        }
        assert_eq!(
            app_state.load().expect("app state").binding,
            files.binding,
            "a failed reconnect never rewrites the locator"
        );
    }

    #[test]
    fn reconnect_requires_an_existing_binding_and_is_idempotent_when_bound() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        marker(dir.path(), &home, &volume());
        let files = bound_files(&home, &volume());
        let volume_source = Arc::new(ToggleVolumeIdentitySource {
            volume: Mutex::new(Some(volume())),
        });
        let lifecycle = compose_lifecycle(
            dir.path(),
            Arc::new(MemoryAppStateStore::new(files.clone())),
            volume_source.clone(),
            probe_report(&volume()),
            FixtureClassification::Clean,
        );
        // Already Bound: reconnect re-verifies the same home_id with zero
        // side effects instead of failing.
        match lifecycle
            .reconnect_same_home()
            .expect("reconnect from Bound")
        {
            BootstrapSnapshot::Bound { home_id, .. } => {
                assert_eq!(home_id.0, HOME_ID);
            }
            other => panic!("expected Bound, got {other:?}"),
        }

        // No binding: reconnect is refused.
        let mut unbound = bound_files(&home, &volume());
        unbound.binding.current = None;
        let lifecycle = compose_lifecycle(
            dir.path(),
            Arc::new(MemoryAppStateStore::new(unbound)),
            volume_source,
            CatalogProbeReport::absent(),
            FixtureClassification::Clean,
        );
        assert!(matches!(
            lifecycle.reconnect_same_home(),
            Err(HomeLifecycleError::ReconnectNotAvailable)
        ));
    }

    #[test]
    fn plan_abandon_is_eligible_in_every_bound_state_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        marker(dir.path(), &home, &volume());
        let files = bound_files(&home, &volume());

        // Bound: eligible.
        let app_state = Arc::new(MemoryAppStateStore::new(files.clone()));
        let volume_source = Arc::new(ToggleVolumeIdentitySource {
            volume: Mutex::new(Some(volume())),
        });
        let lifecycle = compose_lifecycle(
            dir.path(),
            app_state.clone(),
            volume_source.clone(),
            probe_report(&volume()),
            FixtureClassification::Clean,
        );
        let preview = lifecycle.plan_abandon().expect("plan");
        assert_eq!(preview.home_id.0, HOME_ID);
        assert_eq!(preview.path, home);

        // HomeUnavailable: eligible.
        volume_source.set(None);
        lifecycle.plan_abandon().expect("plan from unavailable");

        // HomeIdentityMismatch: eligible.
        let mismatch_volume = VolumeIdentity {
            fsid: "other-fsid".into(),
            uuid: "uuid-1".into(),
        };
        let lifecycle = compose_lifecycle(
            dir.path(),
            app_state.clone(),
            Arc::new(ToggleVolumeIdentitySource {
                volume: Mutex::new(Some(mismatch_volume)),
            }),
            probe_report(&volume()),
            FixtureClassification::Clean,
        );
        lifecycle.plan_abandon().expect("plan from mismatch");

        // Bound fixture lock: eligible.
        let lifecycle = compose_lifecycle(
            dir.path(),
            app_state.clone(),
            Arc::new(ToggleVolumeIdentitySource {
                volume: Mutex::new(Some(volume())),
            }),
            probe_report(&volume()),
            FixtureClassification::Pure,
        );
        lifecycle.plan_abandon().expect("plan from bound lock");

        // No binding: refused.
        let mut files = bound_files(&home, &volume());
        files.binding.current = None;
        let lifecycle = compose_lifecycle(
            dir.path(),
            Arc::new(MemoryAppStateStore::new(files)),
            Arc::new(ToggleVolumeIdentitySource {
                volume: Mutex::new(None),
            }),
            CatalogProbeReport::absent(),
            FixtureClassification::Clean,
        );
        assert!(matches!(
            lifecycle.plan_abandon(),
            Err(HomeLifecycleError::NotAbandonable(_))
        ));

        // Active operation: blocked, not bypassable.
        let mut files = bound_files(&home, &volume());
        files.recovery_ledger.active = Some(RecoveryOperationRecord {
            operation_id: "op-1".into(),
            kind: "fixture_recovery".into(),
            home_id: Some(HomeId(HOME_ID.into())),
            live_path: Some(home.clone()),
            snapshot_path: None,
            prepared_path: None,
            manifest_hash: None,
            external_probe: None,
            cursor: Some("confirmed".into()),
            commit_point: None,
            created_at: "2026-08-01T00:00:00Z".into(),
        });
        let lifecycle = compose_lifecycle(
            dir.path(),
            Arc::new(MemoryAppStateStore::new(files)),
            Arc::new(ToggleVolumeIdentitySource {
                volume: Mutex::new(Some(volume())),
            }),
            probe_report(&volume()),
            FixtureClassification::Clean,
        );
        assert!(matches!(
            lifecycle.plan_abandon(),
            Err(HomeLifecycleError::ActiveOperation { .. })
        ));
    }

    #[test]
    fn apply_abandon_is_high_friction_and_the_cas_is_the_commit_point() {
        let dir = tempfile::tempdir().expect("temp dir");
        // The abandoned Home sits at the default path: after the CAS the
        // bootstrap must surface the Abandoned route, not an orphan.
        let home = dir.path().join("default-home");
        let filesystem = MacOsFileSystem::new(dir.path().to_path_buf());
        filesystem.ensure_directory(&home).expect("home dir");
        filesystem
            .write_utf8_file(&home.join("user-data.txt"), "keep me")
            .expect("user file");
        marker(dir.path(), &home, &volume());
        let files = bound_files(&home, &volume());
        let app_state = Arc::new(MemoryAppStateStore::new(files));
        let volume_source = Arc::new(ToggleVolumeIdentitySource {
            volume: Mutex::new(Some(volume())),
        });
        let lifecycle = compose_lifecycle(
            dir.path(),
            app_state.clone(),
            volume_source.clone(),
            probe_report(&volume()),
            FixtureClassification::Clean,
        );

        // A second service plans the same current binding before either
        // applies: the genuine CAS race.
        let racer = compose_lifecycle(
            dir.path(),
            app_state.clone(),
            volume_source,
            probe_report(&volume()),
            FixtureClassification::Clean,
        );
        let preview = lifecycle.plan_abandon().expect("plan");
        let racer_preview = racer.plan_abandon().expect("racing plan");

        // Wrong typed confirmation: refused, nothing written.
        let wrong = HomeId("00000000-0000-4000-8000-000000000000".into());
        assert!(matches!(
            lifecycle.apply_abandon(&preview.plan_token, &wrong),
            Err(HomeLifecycleError::ConfirmationMismatch)
        ));
        assert!(
            app_state
                .load()
                .expect("app state")
                .binding
                .current
                .is_some(),
            "a rejected confirmation never touches the locator"
        );

        // Correct confirmation: current moves into history, Home untouched.
        let snapshot = lifecycle
            .apply_abandon(&preview.plan_token, &HomeId(HOME_ID.into()))
            .expect("apply abandon");
        assert!(
            matches!(snapshot, BootstrapSnapshot::Abandoned { .. }),
            "the abandoned site at the default path is the Abandoned route, got {snapshot:?}"
        );
        let binding = app_state.load().expect("app state").binding;
        assert!(binding.current.is_none(), "no current binding remains");
        assert_eq!(binding.abandoned.len(), 1);
        assert_eq!(binding.abandoned[0].home_id.0, HOME_ID);
        assert_eq!(binding.abandoned[0].path, home);
        assert!(
            home.join("user-data.txt").is_file(),
            "Abandon never deletes the old Home"
        );

        // The racing apply loses the CAS: the history is never
        // double-recorded and the plan token is consumed only once.
        assert!(matches!(
            racer.apply_abandon(&racer_preview.plan_token, &HomeId(HOME_ID.into())),
            Err(HomeLifecycleError::CasConflict(_))
        ));
        // Reusing the winning plan token is stale.
        assert!(matches!(
            lifecycle.apply_abandon(&preview.plan_token, &HomeId(HOME_ID.into())),
            Err(HomeLifecycleError::PlanStale)
        ));
        let binding = app_state.load().expect("app state").binding;
        assert_eq!(binding.abandoned.len(), 1, "the CAS never double-records");
    }

    #[test]
    fn apply_abandon_requires_a_current_plan_binding_match() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        marker(dir.path(), &home, &volume());
        let files = bound_files(&home, &volume());
        let app_state = Arc::new(MemoryAppStateStore::new(files));
        let volume_source = Arc::new(ToggleVolumeIdentitySource {
            volume: Mutex::new(Some(volume())),
        });
        let lifecycle = compose_lifecycle(
            dir.path(),
            app_state.clone(),
            volume_source,
            probe_report(&volume()),
            FixtureClassification::Clean,
        );
        let preview = lifecycle.plan_abandon().expect("plan");
        // The binding is replaced before apply: the CAS must refuse.
        let mut replaced = bound_files(&home, &volume());
        replaced.binding.current.as_mut().unwrap().home_id =
            HomeId("c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into());
        app_state
            .write_locator(&replaced.binding)
            .expect("replace locator");
        assert!(matches!(
            lifecycle.apply_abandon(&preview.plan_token, &HomeId(HOME_ID.into())),
            Err(HomeLifecycleError::CasConflict(_))
        ));
        assert_eq!(
            app_state
                .load()
                .expect("app state")
                .binding
                .current
                .expect("current")
                .home_id
                .0,
            "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab"
        );
    }
}
