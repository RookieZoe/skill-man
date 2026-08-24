//! Existing Home Recovery integration seam: a lost bootstrap locator may be
//! inspected read-only, while the complete existing Home stays byte-for-byte
//! untouched until the later confirmation ticket owns the locator CAS.

use std::sync::Arc;

use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::volume_identity::MacOsVolumeIdentitySource;
use skill_man_lib::core::bootstrap::{BootstrapConfig, BootstrapService, BootstrapSnapshot};
use skill_man_lib::core::existing_home_recovery::{
    ExistingHomeRecoveryConfig, ExistingHomeRecoveryService,
};
use skill_man_lib::core::fixture_recovery::SystemFixtureClassifier;
use skill_man_lib::core::home::{BoundHome, HomeId, HomeMarker};
use skill_man_lib::core::home_binding::STANDARD_LAYOUT_DIRS;
use skill_man_lib::seams::app_state_store::AppStateStore;
use skill_man_lib::seams::catalog_probe::CatalogProbe;

const HOME_ID: &str = "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
const CREATED_AT: &str = "2026-08-01T00:00:00Z";
const CATALOG_FILE_NAME: &str = "skill-man.sqlite3";

#[test]
fn lost_locator_can_preview_a_complete_custom_existing_home_without_writes() {
    let dir = tempfile::tempdir().expect("temporary root");
    let home_path = dir.path().join("Recovered Home");
    let state_dir = dir.path().join("app-state");
    std::fs::create_dir_all(&home_path).expect("create Home");
    for directory in STANDARD_LAYOUT_DIRS {
        std::fs::create_dir_all(home_path.join(directory)).expect("create standard Home layout");
    }
    std::fs::create_dir_all(&state_dir).expect("create App-level state");

    let home = BoundHome {
        home_id: HomeId(HOME_ID.into()),
        path: home_path.clone(),
        volume_fsid: "historical-fsid".into(),
        volume_uuid: "historical-volume-uuid".into(),
        bound_at: CREATED_AT.into(),
    };
    std::fs::write(
        home_path.join(HomeMarker::FILE_NAME),
        serde_json::to_string_pretty(&HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: home.home_id.clone(),
            volume_fsid: "unrelated-marker-fsid".into(),
            volume_uuid: "unrelated-marker-volume-uuid".into(),
            created_at: CREATED_AT.into(),
        })
        .expect("serialize marker"),
    )
    .expect("write marker");
    SqliteCatalogStore::create_bound(&home, &home_path.join(CATALOG_FILE_NAME))
        .expect("create bound Catalog");
    // Recovery eligibility follows actual capabilities, not a mutable version
    // declaration. The normal Bound Home flow remains intentionally stricter.
    rusqlite::Connection::open(home_path.join(CATALOG_FILE_NAME))
        .expect("open Catalog for test metadata")
        .execute(
            "UPDATE catalog_meta SET schema_version = 999 WHERE singleton = 1",
            [],
        )
        .expect("write future schema metadata");

    let filesystem = Arc::new(MacOsFileSystem::new(dir.path().to_path_buf()));
    let app_state = Arc::new(AppStateStoreFileSystem::new(state_dir.clone()));
    let probe = Arc::new(SqliteCatalogProbe::new());
    let classifier = Arc::new(SystemFixtureClassifier::new(
        probe.clone(),
        filesystem.clone(),
        CATALOG_FILE_NAME.into(),
    ));
    let bootstrap = Arc::new(BootstrapService::new(
        app_state.clone(),
        Arc::new(MacOsVolumeIdentitySource::new()),
        probe.clone(),
        filesystem.clone(),
        classifier.clone(),
        BootstrapConfig {
            state_dir: state_dir.clone(),
            default_home_path: dir.path().join("default Home"),
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    ));
    let service = ExistingHomeRecoveryService::new(
        app_state.clone(),
        bootstrap.clone(),
        probe,
        filesystem,
        classifier,
        ExistingHomeRecoveryConfig {
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    );

    assert_eq!(bootstrap.inspect(), BootstrapSnapshot::Unconfigured);
    assert!(
        SqliteCatalogProbe::new()
            .probe_recovery_profile(&home_path.join(CATALOG_FILE_NAME))
            .expect("read-only Recovery Profile")
            .required_capabilities,
        "capability scan ignores schema_version metadata"
    );
    let marker_before = std::fs::read(home_path.join(HomeMarker::FILE_NAME)).expect("marker bytes");
    let catalog_before = std::fs::read(home_path.join(CATALOG_FILE_NAME)).expect("Catalog bytes");
    let state_before = app_state.load().expect("valid empty state");

    let plan = service
        .prepare(&home_path)
        .expect("complete existing Home produces a Recovery Profile");

    assert_eq!(plan.path, home_path.canonicalize().expect("canonical Home"));
    assert_eq!(plan.home_id, HomeId(HOME_ID.into()));
    assert_eq!(plan.created_at, CREATED_AT);
    assert!(plan.facts.marker_catalog_identity);
    assert!(plan.facts.standard_layout);
    assert!(plan.facts.catalog_integrity);
    assert!(plan.facts.catalog_foreign_keys);
    assert!(plan.facts.catalog_capabilities);
    assert!(!plan.plan_token.is_empty());
    assert_eq!(
        std::fs::read(home_path.join(HomeMarker::FILE_NAME)).expect("marker still exists"),
        marker_before
    );
    assert_eq!(
        std::fs::read(home_path.join(CATALOG_FILE_NAME)).expect("Catalog still exists"),
        catalog_before
    );
    assert_eq!(
        app_state.load().expect("state remains readable"),
        state_before
    );

    service.cancel(&plan.plan_token).expect("cancel preview");
    std::fs::remove_dir(home_path.join("cache")).expect("remove required cache root");
    assert!(matches!(
        service.prepare(&home_path),
        Err(skill_man_lib::core::existing_home_recovery::ExistingHomeRecoveryError::ProfileRejected {
            reason: skill_man_lib::core::existing_home_recovery::RecoveryProfileRejection::LayoutCapabilities,
        })
    ));
    assert_eq!(
        std::fs::read(home_path.join(HomeMarker::FILE_NAME)).expect("marker remains unchanged"),
        marker_before
    );
    assert_eq!(
        std::fs::read(home_path.join(CATALOG_FILE_NAME)).expect("Catalog remains unchanged"),
        catalog_before
    );
    assert_eq!(
        app_state.load().expect("state remains unchanged"),
        state_before
    );
    std::fs::create_dir(home_path.join("cache")).expect("restore required cache root");
    std::fs::create_dir_all(home_path.join("staging/orphaned-operation"))
        .expect("write unfinished staging test fixture");
    assert!(matches!(
        service.prepare(&home_path),
        Err(skill_man_lib::core::existing_home_recovery::ExistingHomeRecoveryError::ProfileRejected {
            reason: skill_man_lib::core::existing_home_recovery::RecoveryProfileRejection::OperationRecoveryRequired,
        })
    ));
    assert_eq!(
        app_state.load().expect("state stays unchanged"),
        state_before
    );

    // An empty Catalog can pass `foreign_key_check` even after a source
    // constraint is stripped. Recovery Profile must inspect the actual
    // runtime constraint surface, and must still leave the partial Catalog
    // untouched when it rejects it.
    let connection = rusqlite::Connection::open(home_path.join(CATALOG_FILE_NAME))
        .expect("open Catalog to create partial-profile fixture");
    connection
        .execute_batch(
            "
            ALTER TABLE remote_bindings RENAME TO incomplete_remote_bindings;
            CREATE TABLE remote_bindings (
                skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                remote_id TEXT NOT NULL,
                requested_ref TEXT NOT NULL,
                verification_anchor_commit TEXT NOT NULL,
                original_commit_known INTEGER NOT NULL DEFAULT 0 CHECK (original_commit_known IN (0, 1)),
                skill_path TEXT NOT NULL,
                provider_hash TEXT,
                remote_baseline_hash TEXT NOT NULL,
                current_baseline_hash TEXT NOT NULL,
                last_checked_at INTEGER,
                last_updated_at INTEGER
            );
            DROP TABLE incomplete_remote_bindings;
            ",
        )
        .expect("remove the remote parent foreign-key constraint");
    drop(connection);
    let partial_catalog_before =
        std::fs::read(home_path.join(CATALOG_FILE_NAME)).expect("partial Catalog bytes");
    assert!(matches!(
        service.prepare(&home_path),
        Err(skill_man_lib::core::existing_home_recovery::ExistingHomeRecoveryError::ProfileRejected {
            reason: skill_man_lib::core::existing_home_recovery::RecoveryProfileRejection::CatalogCapabilities,
        })
    ));
    assert_eq!(
        std::fs::read(home_path.join(CATALOG_FILE_NAME))
            .expect("partial Catalog remains unchanged"),
        partial_catalog_before
    );
    assert_eq!(
        app_state.load().expect("state stays unchanged"),
        state_before
    );
}
