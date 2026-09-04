//! Default-path Existing Home Recovery at the public Bootstrap and recovery
//! service seams. The offer is read-only until direct confirmation; unsafe
//! default evidence stays closed instead of falling into Home Binding.

use std::path::Path;
use std::sync::Arc;

use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::volume_identity::MacOsVolumeIdentitySource;
use skill_man_lib::core::bootstrap::{
    BootstrapConfig, BootstrapService, BootstrapSnapshot, DefaultHomeRecoveryBlockedReason,
};
use skill_man_lib::core::existing_home_recovery::{
    ExistingHomeRecoveryConfig, ExistingHomeRecoveryError, ExistingHomeRecoveryService,
    RecoveryEligibilityRejection,
};
use skill_man_lib::core::fixture_recovery::SystemFixtureClassifier;
use skill_man_lib::core::home::{BoundHome, HomeId, HomeMarker};
use skill_man_lib::core::home_binding::STANDARD_LAYOUT_DIRS;
use skill_man_lib::core::write_gate::WriteGate;
use skill_man_lib::seams::app_state_store::AppStateStore;
use skill_man_lib::seams::volume_identity::VolumeIdentitySource;

const CATALOG_FILE_NAME: &str = "skill-man.sqlite3";
const CREATED_AT: &str = "2026-08-01T00:00:00Z";
const HOME_ID: &str = "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";

fn create_complete_home(path: &Path, home_id: &str) {
    std::fs::create_dir_all(path).expect("create Home");
    for directory in STANDARD_LAYOUT_DIRS {
        std::fs::create_dir_all(path.join(directory)).expect("create Home layout");
    }
    let volume_source = MacOsVolumeIdentitySource::new();
    let volume = volume_source
        .volume_identity(path)
        .expect("read volume identity")
        .expect("Home has a volume");
    let home = BoundHome {
        home_id: HomeId(home_id.into()),
        path: path.to_path_buf(),
        volume_fsid: volume.fsid.clone(),
        volume_uuid: volume.uuid.clone(),
        bound_at: CREATED_AT.into(),
    };
    std::fs::write(
        path.join(HomeMarker::FILE_NAME),
        serde_json::to_string_pretty(&HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: home.home_id.clone(),
            volume_fsid: volume.fsid,
            volume_uuid: volume.uuid,
            created_at: CREATED_AT.into(),
        })
        .expect("serialize marker"),
    )
    .expect("write marker");
    SqliteCatalogStore::create_bound(&home, &path.join(CATALOG_FILE_NAME))
        .expect("create bound Catalog");
}

fn compose(
    root: &Path,
    default_home: &Path,
) -> (
    Arc<AppStateStoreFileSystem>,
    Arc<BootstrapService>,
    ExistingHomeRecoveryService,
) {
    let state_dir = root.join("app-state");
    std::fs::create_dir_all(&state_dir).expect("create app state");
    let filesystem = Arc::new(MacOsFileSystem::new(root.to_path_buf()));
    let app_state = Arc::new(AppStateStoreFileSystem::new(state_dir.clone()));
    let volume = Arc::new(MacOsVolumeIdentitySource::new());
    let probe = Arc::new(SqliteCatalogProbe::new());
    let classifier = Arc::new(SystemFixtureClassifier::new(
        probe.clone(),
        filesystem.clone(),
        CATALOG_FILE_NAME.into(),
    ));
    let bootstrap = Arc::new(BootstrapService::new(
        app_state.clone(),
        volume.clone(),
        probe.clone(),
        filesystem.clone(),
        classifier.clone(),
        BootstrapConfig {
            state_dir,
            default_home_path: default_home.to_path_buf(),
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    ));
    let recovery = ExistingHomeRecoveryService::new(
        app_state.clone(),
        bootstrap.clone(),
        volume,
        probe,
        filesystem,
        classifier,
        Arc::new(WriteGate::open_for_tests()),
        ExistingHomeRecoveryConfig {
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    );
    (app_state, bootstrap, recovery)
}

#[test]
fn default_complete_home_offers_read_only_recovery_and_cancel_keeps_the_offer() {
    let root = tempfile::tempdir().expect("temporary root");
    let default_home = root.path().join("Default Home");
    create_complete_home(&default_home, HOME_ID);
    let custom_home = root.path().join("Custom Home");
    create_complete_home(&custom_home, "41d8b95a-51b8-46aa-99d9-9f0290c9d8ef");
    let (app_state, bootstrap, recovery) = compose(root.path(), &default_home);

    assert!(matches!(
        bootstrap.inspect(),
        BootstrapSnapshot::DefaultHomeRecoveryOffer { ref path }
            if path == &default_home.canonicalize().expect("canonical default")
    ));
    let marker_before =
        std::fs::read(default_home.join(HomeMarker::FILE_NAME)).expect("marker bytes before");
    let catalog_before =
        std::fs::read(default_home.join(CATALOG_FILE_NAME)).expect("Catalog bytes before");
    let state_before = app_state.load().expect("empty app state");

    let plan = recovery
        .prepare(&default_home)
        .expect("prepare default Offer");
    assert_eq!(app_state.load().expect("state after preview"), state_before);
    assert_eq!(
        std::fs::read(default_home.join(HomeMarker::FILE_NAME)).expect("marker after preview"),
        marker_before
    );
    assert_eq!(
        std::fs::read(default_home.join(CATALOG_FILE_NAME)).expect("Catalog after preview"),
        catalog_before
    );
    recovery.cancel(&plan.plan_token).expect("cancel preview");
    assert!(matches!(
        bootstrap.inspect(),
        BootstrapSnapshot::DefaultHomeRecoveryOffer { .. }
    ));

    // A default Offer is not an authorization to scan or recover another
    // selected path through the public recovery service.
    assert!(matches!(
        recovery.prepare(&custom_home),
        Err(ExistingHomeRecoveryError::Ineligible {
            reason: RecoveryEligibilityRejection::BootstrapState,
        })
    ));

    let confirmed = recovery
        .prepare(&default_home)
        .expect("prepare default Offer again");
    assert!(matches!(
        recovery.confirm(&confirmed.plan_token),
        Ok(BootstrapSnapshot::Bound { ref home_id, .. }) if home_id.0 == HOME_ID
    ));
    assert_eq!(
        std::fs::read(default_home.join(HomeMarker::FILE_NAME)).expect("marker after confirm"),
        marker_before
    );
    assert_eq!(
        std::fs::read(default_home.join(CATALOG_FILE_NAME)).expect("Catalog after confirm"),
        catalog_before
    );
}

#[test]
fn unfinished_default_operation_is_blocked_and_never_becomes_a_recovery_offer() {
    let root = tempfile::tempdir().expect("temporary root");
    let default_home = root.path().join("Default Home");
    create_complete_home(&default_home, HOME_ID);
    std::fs::create_dir(default_home.join("operations/incomplete"))
        .expect("unfinished operation evidence");
    let (app_state, bootstrap, recovery) = compose(root.path(), &default_home);
    let state_before = app_state.load().expect("empty app state");

    assert!(matches!(
        bootstrap.inspect(),
        BootstrapSnapshot::DefaultHomeRecoveryBlocked {
            reason: DefaultHomeRecoveryBlockedReason::OperationRecoveryRequired,
            ..
        }
    ));
    assert!(matches!(
        recovery.prepare(&default_home),
        Err(ExistingHomeRecoveryError::ProfileRejected { .. })
    ));
    assert_eq!(
        app_state.load().expect("blocked route has no writes"),
        state_before
    );
}

#[test]
fn default_catalog_evidence_that_fails_the_profile_is_blocked_not_fixture_recovery() {
    let root = tempfile::tempdir().expect("temporary root");
    let default_home = root.path().join("Default Home");
    create_complete_home(&default_home, HOME_ID);
    std::fs::remove_file(default_home.join(CATALOG_FILE_NAME)).expect("remove Catalog");
    let (app_state, bootstrap, recovery) = compose(root.path(), &default_home);
    let state_before = app_state.load().expect("empty app state");

    assert!(matches!(
        bootstrap.inspect(),
        BootstrapSnapshot::DefaultHomeRecoveryBlocked {
            reason: DefaultHomeRecoveryBlockedReason::CatalogMissing,
            ..
        }
    ));
    assert!(matches!(
        recovery.prepare(&default_home),
        Err(ExistingHomeRecoveryError::ProfileRejected { .. })
    ));
    assert_eq!(
        app_state.load().expect("blocked route has no writes"),
        state_before
    );
}

#[test]
fn unreadable_default_path_fails_closed_instead_of_reopening_first_binding() {
    let root = tempfile::tempdir().expect("temporary root");
    let default_home = root.path().join("Default Home");
    std::os::unix::fs::symlink(&default_home, &default_home)
        .expect("create self-referential default path");
    let (_, bootstrap, _) = compose(root.path(), &default_home);

    assert!(matches!(
        bootstrap.inspect(),
        BootstrapSnapshot::DefaultHomeRecoveryBlocked {
            reason: DefaultHomeRecoveryBlockedReason::Unreadable,
            ..
        }
    ));
}

#[test]
fn a_custom_plan_cannot_confirm_through_a_default_offer_that_appears_later() {
    let root = tempfile::tempdir().expect("temporary root");
    let default_home = root.path().join("Default Home");
    let custom_home = root.path().join("Custom Home");
    create_complete_home(&custom_home, "41d8b95a-51b8-46aa-99d9-9f0290c9d8ef");
    let (app_state, bootstrap, recovery) = compose(root.path(), &default_home);
    let custom_plan = recovery
        .prepare(&custom_home)
        .expect("prepare custom Home before a default Offer exists");

    create_complete_home(&default_home, HOME_ID);
    assert!(matches!(
        bootstrap.inspect(),
        BootstrapSnapshot::DefaultHomeRecoveryOffer { .. }
    ));
    assert!(matches!(
        recovery.confirm(&custom_plan.plan_token),
        Err(ExistingHomeRecoveryError::PlanStale)
    ));
    assert!(
        app_state
            .load()
            .expect("blocked confirmation leaves the locator empty")
            .binding
            .current
            .is_none()
    );
}
