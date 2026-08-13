//! Bootstrap authority integration tests (spec §3.3, §5.1, §10.1): the
//! production startup path — App-level state, volume identity, Home marker
//! and a read-only Catalog probe — resolved before any writable open, with
//! zero auto-writes and zero fixture fallback.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::BoundTestHome;
use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::core::bootstrap::{
    BootstrapConfig, BootstrapService, BootstrapSnapshot, CatalogAccess,
};
use skill_man_lib::core::home::{HomeId, HomeMarker, VolumeIdentity};
use skill_man_lib::core::write_gate::{ReadOnlyReason, WriteGateState};
use skill_man_lib::seams::app_state_store::AppStateStore;
use skill_man_lib::seams::catalog_probe::CatalogProbe;
use skill_man_lib::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

struct FixedVolumeIdentity(Option<VolumeIdentity>);

impl VolumeIdentitySource for FixedVolumeIdentity {
    fn volume_identity(
        &self,
        _path: &std::path::Path,
    ) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
        Ok(self.0.clone())
    }
}

fn bootstrap_for(home: &BoundTestHome, volume: Option<VolumeIdentity>) -> BootstrapService {
    BootstrapService::new(
        Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
        Arc::new(FixedVolumeIdentity(volume)),
        Arc::new(SqliteCatalogProbe::new()),
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        BootstrapConfig {
            state_dir: home.state_dir.clone(),
            default_home_path: home.library_root.clone(),
            catalog_file_name: common::CATALOG_FILE_NAME.into(),
        },
    )
}

fn test_volume() -> VolumeIdentity {
    VolumeIdentity {
        fsid: "test-fsid".into(),
        uuid: "test-uuid".into(),
    }
}

#[test]
fn fresh_machine_is_unconfigured_with_zero_home_artifacts() {
    let dir = tempfile::tempdir().expect("temporary home");
    let home = dir.path().join("Library/Application Support/skill-man");
    let state_dir = dir
        .path()
        .join("Library/Application Support/skill-man-state");
    let service = BootstrapService::new(
        Arc::new(AppStateStoreFileSystem::new(state_dir.clone())),
        Arc::new(FixedVolumeIdentity(None)),
        Arc::new(SqliteCatalogProbe::new()),
        Arc::new(MacOsFileSystem::new(dir.path().to_path_buf())),
        BootstrapConfig {
            state_dir,
            default_home_path: home.clone(),
            catalog_file_name: "skill-man.sqlite3".into(),
        },
    );

    assert_eq!(service.inspect(), BootstrapSnapshot::Unconfigured);
    assert!(service.verified_bound_home().is_none());
    assert!(
        !home.exists(),
        "Unconfigured must not create any Home or SQLite artifact"
    );
    assert!(
        !dir.path()
            .join("Library/Application Support/skill-man-state")
            .exists(),
        "Unconfigured must not create the app state directory either"
    );
}

#[test]
fn matching_locator_marker_catalog_and_volume_resolve_bound_read_write() {
    let home = BoundTestHome::new();
    let service = bootstrap_for(&home, Some(test_volume()));

    let snapshot = service.inspect();
    match &snapshot {
        BootstrapSnapshot::Bound {
            home_id,
            catalog_access: CatalogAccess::ReadWrite,
            snapshot_version,
        } => {
            assert_eq!(home_id.0, common::HOME_ID);
            assert_eq!(*snapshot_version, 0, "fresh bound Catalog snapshot");
        }
        other => panic!("expected Bound ReadWrite, got {other:?}"),
    }

    // The verified value object reopens the same Catalog through open_bound.
    let bound = service.verified_bound_home().expect("verified Home");
    assert_eq!(bound.home_id.0, common::HOME_ID);
    assert_eq!(bound.path, home.library_root);
    let opened = SqliteCatalogStore::open_bound(&bound, &home.catalog_path())
        .expect("writable open after verification");
    assert_eq!(
        opened.startup_status().access,
        skill_man_lib::seams::catalog_store::StartupAccess::ReadWrite
    );
    assert!(matches!(
        snapshot.write_gate_state(Some(&bound)),
        WriteGateState::Open(_)
    ));
}

#[test]
fn missing_marker_is_home_identity_mismatch_with_zero_auto_writes() {
    let home = BoundTestHome::new();
    std::fs::remove_file(home.library_root.join(HomeMarker::FILE_NAME))
        .expect("remove marker to simulate a foreign Home");

    let before = std::fs::read_to_string(home.state_dir.join("home-binding.json"))
        .expect("locator before inspect");
    let service = bootstrap_for(&home, Some(test_volume()));
    let snapshot = service.inspect();
    match snapshot {
        BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
            assert_eq!(
                diagnostic.as_ref().map(|d| d.code.as_str()),
                Some("home_marker_invalid")
            );
        }
        other => panic!("expected HomeIdentityMismatch, got {other:?}"),
    }
    // Zero auto-writes: the locator is untouched and no marker was rebuilt.
    let after = std::fs::read_to_string(home.state_dir.join("home-binding.json"))
        .expect("locator after inspect");
    assert_eq!(before, after);
    assert!(
        !home.library_root.join(HomeMarker::FILE_NAME).exists(),
        "the missing marker must not be recreated"
    );
}

#[test]
fn mismatched_volume_is_home_identity_mismatch_not_unavailable() {
    let home = BoundTestHome::new();
    let service = bootstrap_for(
        &home,
        Some(VolumeIdentity {
            fsid: "other-fsid".into(),
            uuid: "other-uuid".into(),
        }),
    );
    match service.inspect() {
        BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
            assert_eq!(
                diagnostic.as_ref().map(|d| d.code.as_str()),
                Some("volume_identity_mismatch")
            );
        }
        other => panic!("expected HomeIdentityMismatch, got {other:?}"),
    }
}

#[test]
fn missing_catalog_is_home_identity_mismatch() {
    let home = BoundTestHome::new();
    std::fs::remove_file(home.catalog_path()).expect("remove Catalog");
    let service = bootstrap_for(&home, Some(test_volume()));
    match service.inspect() {
        BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
            assert_eq!(
                diagnostic.as_ref().map(|d| d.code.as_str()),
                Some("catalog_missing")
            );
        }
        other => panic!("expected HomeIdentityMismatch, got {other:?}"),
    }
}

#[test]
fn corrupt_catalog_reports_the_real_diagnostic_and_never_serves_fixture_skills() {
    // Close every Catalog handle first: a live connection would serve stale
    // page-cache reads instead of the corrupted file (SQLite behavior), which
    // production cannot hit because bootstrap probes before any open.
    let BoundTestHome {
        dir,
        home: _,
        state_dir,
        library_root,
        sqlite,
        runtime,
        filesystem: _,
        write_gate,
    } = BoundTestHome::new();
    drop(sqlite);
    drop(runtime);
    let _ = write_gate;
    let catalog_path = library_root.join(common::CATALOG_FILE_NAME);
    std::fs::write(&catalog_path, b"this is not a sqlite database at all")
        .expect("corrupt the Catalog file");
    let service = BootstrapService::new(
        Arc::new(AppStateStoreFileSystem::new(state_dir)),
        Arc::new(FixedVolumeIdentity(Some(test_volume()))),
        Arc::new(SqliteCatalogProbe::new()),
        Arc::new(MacOsFileSystem::new(dir.path().to_path_buf())),
        BootstrapConfig {
            state_dir: dir
                .path()
                .join("Library/Application Support/skill-man-state"),
            default_home_path: library_root.clone(),
            catalog_file_name: common::CATALOG_FILE_NAME.into(),
        },
    );
    let snapshot = service.inspect();
    match &snapshot {
        BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
            let diagnostic = diagnostic.as_ref().expect("real diagnostic");
            assert_eq!(diagnostic.code, "catalog_unreadable");
            assert!(
                !diagnostic.message.is_empty(),
                "the raw diagnostic must carry the open failure detail"
            );
        }
        other => panic!("expected HomeIdentityMismatch, got {other:?}"),
    }
    // No fixture fallback: the snapshot is a closed state, so no catalog
    // surface exists to serve skill-authoring/media-xray/legacy-audit from.
    assert!(!snapshot.is_bound());
    assert!(service.verified_bound_home().is_none());
}

#[test]
fn future_schema_with_matching_identity_is_bound_read_only() {
    let home = BoundTestHome::new();
    home.with_sql("bump schema", |connection| {
        connection
            .execute(
                "UPDATE catalog_meta SET schema_version = 9 WHERE singleton = 1",
                [],
            )
            .expect("bump schema version");
    });
    let service = bootstrap_for(&home, Some(test_volume()));
    match service.inspect() {
        BootstrapSnapshot::Bound {
            catalog_access: CatalogAccess::ReadOnly { reason },
            ..
        } => assert_eq!(reason, ReadOnlyReason::UnsupportedSchema),
        other => panic!("expected Bound ReadOnly, got {other:?}"),
    }
}

#[test]
fn legacy_path_without_binding_is_legacy_detected_read_only() {
    let dir = tempfile::tempdir().expect("temporary home");
    let legacy_home = dir.path().join("Library/Application Support/skill-man");
    std::fs::create_dir_all(&legacy_home).expect("legacy Home exists");
    let service = BootstrapService::new(
        Arc::new(AppStateStoreFileSystem::new(
            dir.path()
                .join("Library/Application Support/skill-man-state"),
        )),
        Arc::new(FixedVolumeIdentity(None)),
        Arc::new(SqliteCatalogProbe::new()),
        Arc::new(MacOsFileSystem::new(dir.path().to_path_buf())),
        BootstrapConfig {
            state_dir: dir
                .path()
                .join("Library/Application Support/skill-man-state"),
            default_home_path: legacy_home.clone(),
            catalog_file_name: "skill-man.sqlite3".into(),
        },
    );
    match service.inspect() {
        BootstrapSnapshot::LegacyDetected { path } => assert_eq!(path, legacy_home),
        other => panic!("expected LegacyDetected, got {other:?}"),
    }
    assert!(
        service.verified_bound_home().is_none(),
        "Legacy is never treated as Bound"
    );
}

#[test]
fn invalid_locator_is_app_state_unavailable_not_unconfigured() {
    let dir = tempfile::tempdir().expect("temporary home");
    let state_dir = dir
        .path()
        .join("Library/Application Support/skill-man-state");
    std::fs::create_dir_all(&state_dir).expect("state dir");
    std::fs::write(
        state_dir.join("home-binding.json"),
        r#"{"schema_version": 1, "current": {"home_id": "not-a-uuid"}}"#,
    )
    .expect("corrupt locator");
    let service = BootstrapService::new(
        Arc::new(AppStateStoreFileSystem::new(state_dir)),
        Arc::new(FixedVolumeIdentity(None)),
        Arc::new(SqliteCatalogProbe::new()),
        Arc::new(MacOsFileSystem::new(dir.path().to_path_buf())),
        BootstrapConfig {
            state_dir: dir
                .path()
                .join("Library/Application Support/skill-man-state"),
            default_home_path: dir.path().join("skill-man"),
            catalog_file_name: "skill-man.sqlite3".into(),
        },
    );
    match service.inspect() {
        BootstrapSnapshot::AppStateUnavailable { diagnostic } => {
            assert_eq!(diagnostic.code, "app_state_unavailable");
        }
        other => panic!("expected AppStateUnavailable, got {other:?}"),
    }
}

#[test]
fn unbound_catalog_identity_is_home_identity_mismatch() {
    let dir = tempfile::tempdir().expect("temporary home");
    let home_path = dir.path().join("Library/Application Support/skill-man");
    std::fs::create_dir_all(&home_path).expect("Home root");
    let state_dir = dir
        .path()
        .join("Library/Application Support/skill-man-state");
    std::fs::create_dir_all(&state_dir).expect("state dir");
    let bound = skill_man_lib::core::home::BoundHome {
        home_id: HomeId(common::HOME_ID.into()),
        path: home_path.clone(),
        volume_fsid: "test-fsid".into(),
        volume_uuid: "test-uuid".into(),
        bound_at: "2026-08-01T00:00:00Z".into(),
    };
    // Locator + marker, but the Catalog is a fresh v5 without identity.
    AppStateStoreFileSystem::new(state_dir.clone())
        .write_locator(&skill_man_lib::seams::app_state_store::HomeBindingFile {
            schema_version: 1,
            current: Some(skill_man_lib::seams::app_state_store::HomeBindingRecord {
                home_id: bound.home_id.clone(),
                path: home_path.clone(),
                volume_fsid: bound.volume_fsid.clone(),
                volume_uuid: bound.volume_uuid.clone(),
                bound_at: bound.bound_at.clone(),
            }),
            abandoned: vec![],
        })
        .expect("write locator");
    std::fs::write(
        home_path.join(HomeMarker::FILE_NAME),
        serde_json::to_string(&HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: bound.home_id.clone(),
            volume_fsid: "test-fsid".into(),
            volume_uuid: "test-uuid".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
        })
        .expect("marker JSON"),
    )
    .expect("write marker");
    SqliteCatalogStore::open(&home_path.join("skill-man.sqlite3")).expect("fresh v5 Catalog");

    let service = BootstrapService::new(
        Arc::new(AppStateStoreFileSystem::new(state_dir)),
        Arc::new(FixedVolumeIdentity(Some(test_volume()))),
        Arc::new(SqliteCatalogProbe::new()),
        Arc::new(MacOsFileSystem::new(dir.path().to_path_buf())),
        BootstrapConfig {
            state_dir: dir
                .path()
                .join("Library/Application Support/skill-man-state"),
            default_home_path: home_path.clone(),
            catalog_file_name: "skill-man.sqlite3".into(),
        },
    );
    match service.inspect() {
        BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
            assert_eq!(
                diagnostic.as_ref().map(|d| d.code.as_str()),
                Some("catalog_identity_missing")
            );
        }
        other => panic!("expected HomeIdentityMismatch, got {other:?}"),
    }
    // The unbound Catalog stays untouched: still v5, still no identity.
    let probe = SqliteCatalogProbe::new();
    let report = probe
        .probe(&home_path.join("skill-man.sqlite3"))
        .expect("probe unchanged Catalog");
    assert_eq!(report.schema_version, Some(5));
    assert!(report.home_identity.is_none());
}

#[test]
fn open_bound_refuses_a_catalog_whose_identity_was_rewritten() {
    let home = BoundTestHome::new();
    // A foreign process rewrites the Catalog identity to another Home.
    home.with_sql("rewrite identity", |connection| {
        connection
            .execute(
                "UPDATE catalog_meta
                 SET home_id = 'c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab'
                 WHERE singleton = 1",
                [],
            )
            .expect("rewrite identity");
    });
    let other = skill_man_lib::core::home::BoundHome {
        home_id: HomeId("c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
        path: PathBuf::from("/tmp/other-home"),
        volume_fsid: "test-fsid".into(),
        volume_uuid: "test-uuid".into(),
        bound_at: "2026-08-01T00:00:00Z".into(),
    };
    // The original binding can no longer open the Catalog...
    assert!(matches!(
        SqliteCatalogStore::open_bound(&home.home, &home.catalog_path()),
        Err(skill_man_lib::adapters::sqlite::BoundCatalogOpenError::IdentityMismatch)
    ));
    // ...and the bootstrap authority reports the mismatch.
    let service = bootstrap_for(&home, Some(test_volume()));
    match service.inspect() {
        BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
            assert_eq!(
                diagnostic.as_ref().map(|d| d.code.as_str()),
                Some("catalog_identity_mismatch")
            );
        }
        other => panic!("expected HomeIdentityMismatch, got {other:?}"),
    }
    // The rewritten identity itself still opens under its own binding.
    SqliteCatalogStore::open_bound(&other, &home.catalog_path()).expect("other binding opens");
}
