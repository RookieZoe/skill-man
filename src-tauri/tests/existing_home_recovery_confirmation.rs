//! Existing Home Recovery confirmation integration seam: a complete Home may
//! rebuild only its lost bootstrap locator. The Home itself remains untouched
//! and the normal bootstrap authority immediately recognizes the binding.

use std::sync::Arc;

use rusqlite::Connection;
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
use skill_man_lib::seams::app_state_store::{
    AppStateFiles, AppStateStore, AppStateStoreError, HomeBindingFile, RecoveryLedgerFile,
};
use skill_man_lib::seams::volume_identity::VolumeIdentitySource;

const HOME_ID: &str = "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
const CREATED_AT: &str = "2026-08-01T00:00:00Z";
const CATALOG_FILE_NAME: &str = "skill-man.sqlite3";

#[derive(Clone, Copy)]
enum ConfirmationFault {
    CasConflict,
    PreCommitWriteFailure,
}

/// A fault-injecting AppStateStore sits at the public confirmation seam. It
/// proves that a rejected CAS or an interrupted pre-commit write cannot mutate
/// the selected Home or leave a partial locator behind.
struct FaultingAppStateStore {
    inner: Arc<AppStateStoreFileSystem>,
    fault: ConfirmationFault,
}

impl AppStateStore for FaultingAppStateStore {
    fn load(&self) -> Result<AppStateFiles, AppStateStoreError> {
        self.inner.load()
    }

    fn write_locator(&self, binding: &HomeBindingFile) -> Result<(), AppStateStoreError> {
        self.inner.write_locator(binding)
    }

    fn cas_unconfigured_locator(&self, _next: &HomeBindingFile) -> Result<(), AppStateStoreError> {
        match self.fault {
            ConfirmationFault::CasConflict => Err(AppStateStoreError::LocatorCasConflict {
                expected: None,
                found: None,
            }),
            ConfirmationFault::PreCommitWriteFailure => Err(AppStateStoreError::WriteFailed(
                "injected pre-commit failure".into(),
            )),
        }
    }

    fn write_recovery_ledger(&self, ledger: &RecoveryLedgerFile) -> Result<(), AppStateStoreError> {
        self.inner.write_recovery_ledger(ledger)
    }
}

#[test]
fn confirmation_rebuilds_only_the_lost_locator_and_immediately_binds_the_existing_home() {
    // POSIX record locks do not conflict within one process, so the real WAL
    // quiescence probe requires this test binary to act as a separate, still
    // running Skill Man process.
    if std::env::var("EHR_LOCK_HOLDER_MODE").as_deref() == Ok("1") {
        let database = std::env::var("EHR_LOCK_HOLDER_DB").expect("EHR_LOCK_HOLDER_DB");
        let ready = std::env::var("EHR_LOCK_HOLDER_READY").expect("EHR_LOCK_HOLDER_READY");
        let writer = Connection::open(database).expect("writer connection");
        writer
            .execute_batch(
                "BEGIN IMMEDIATE; UPDATE preferences SET show_in_dock = 0 WHERE singleton = 1;",
            )
            .expect("writer transaction");
        std::fs::write(&ready, b"locked").expect("write ready marker");
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }

    let dir = tempfile::tempdir().expect("temporary root");
    let home_path = dir.path().join("Recovered Home");
    let state_dir = dir.path().join("app-state");
    std::fs::create_dir_all(&home_path).expect("create Home");
    for directory in STANDARD_LAYOUT_DIRS {
        std::fs::create_dir_all(home_path.join(directory)).expect("create standard Home layout");
    }
    std::fs::create_dir_all(&state_dir).expect("create App-level state");

    let volume_source = Arc::new(MacOsVolumeIdentitySource::new());
    let volume = volume_source
        .volume_identity(&home_path)
        .expect("read volume identity")
        .expect("Home is on a volume");
    let home = BoundHome {
        home_id: HomeId(HOME_ID.into()),
        path: home_path.clone(),
        volume_fsid: volume.fsid.clone(),
        volume_uuid: volume.uuid.clone(),
        bound_at: CREATED_AT.into(),
    };
    std::fs::write(
        home_path.join(HomeMarker::FILE_NAME),
        serde_json::to_string_pretty(&HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: home.home_id.clone(),
            volume_fsid: volume.fsid.clone(),
            volume_uuid: volume.uuid.clone(),
            created_at: CREATED_AT.into(),
        })
        .expect("serialize marker"),
    )
    .expect("write marker");
    SqliteCatalogStore::create_bound(&home, &home_path.join(CATALOG_FILE_NAME))
        .expect("create bound Catalog");

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
        volume_source.clone(),
        probe.clone(),
        filesystem.clone(),
        classifier.clone(),
        BootstrapConfig {
            state_dir,
            default_home_path: dir.path().join("default Home"),
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    ));
    let marker_before = std::fs::read(home_path.join(HomeMarker::FILE_NAME)).expect("marker bytes");
    let catalog_before = std::fs::read(home_path.join(CATALOG_FILE_NAME)).expect("Catalog bytes");

    for (fault, expected_error) in [
        (ConfirmationFault::CasConflict, "CAS conflict"),
        (
            ConfirmationFault::PreCommitWriteFailure,
            "pre-commit failure",
        ),
    ] {
        let faulting_store: Arc<dyn AppStateStore> = Arc::new(FaultingAppStateStore {
            inner: app_state.clone(),
            fault,
        });
        let faulting_service = ExistingHomeRecoveryService::new(
            faulting_store,
            bootstrap.clone(),
            volume_source.clone(),
            probe.clone(),
            filesystem.clone(),
            classifier.clone(),
            ExistingHomeRecoveryConfig {
                catalog_file_name: CATALOG_FILE_NAME.into(),
            },
        );
        let fault_plan = faulting_service
            .prepare(&home_path)
            .expect("prepare fault-injection Recovery Plan");
        let failure = faulting_service
            .confirm(&fault_plan.plan_token)
            .expect_err(expected_error);
        match fault {
            ConfirmationFault::CasConflict => assert!(matches!(
                failure,
                skill_man_lib::core::existing_home_recovery::ExistingHomeRecoveryError::PlanStale
            )),
            ConfirmationFault::PreCommitWriteFailure => assert!(matches!(
                failure,
                skill_man_lib::core::existing_home_recovery::ExistingHomeRecoveryError::StateStore(
                    _
                )
            )),
        }
        assert_eq!(
            app_state.load().expect("empty locator after failed CAS"),
            AppStateFiles {
                binding: HomeBindingFile::empty(),
                recovery_ledger: RecoveryLedgerFile::empty(),
            }
        );
        assert_eq!(
            std::fs::read(home_path.join(HomeMarker::FILE_NAME)).expect("marker after failed CAS"),
            marker_before
        );
        assert_eq!(
            std::fs::read(home_path.join(CATALOG_FILE_NAME)).expect("Catalog after failed CAS"),
            catalog_before
        );
    }

    let service = ExistingHomeRecoveryService::new(
        app_state.clone(),
        bootstrap.clone(),
        volume_source,
        probe,
        filesystem,
        classifier,
        ExistingHomeRecoveryConfig {
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    );

    let writer_plan = service
        .prepare(&home_path)
        .expect("prepare Recovery Plan before writer starts");
    let ready_file = dir.path().join("existing-home-recovery-writer-ready");
    let exe = std::env::current_exe().expect("test binary");
    let mut child = std::process::Command::new(exe)
        .arg("--exact")
        .arg("confirmation_rebuilds_only_the_lost_locator_and_immediately_binds_the_existing_home")
        .env("EHR_LOCK_HOLDER_MODE", "1")
        .env("EHR_LOCK_HOLDER_DB", home_path.join(CATALOG_FILE_NAME))
        .env("EHR_LOCK_HOLDER_READY", &ready_file)
        .spawn()
        .expect("spawn WAL writer");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready_file.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the WAL writer never reported ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(matches!(
        service.confirm(&writer_plan.plan_token),
        Err(skill_man_lib::core::existing_home_recovery::ExistingHomeRecoveryError::PlanStale)
    ));
    assert_eq!(
        app_state
            .load()
            .expect("writer-blocked confirmation leaves locator absent"),
        AppStateFiles {
            binding: HomeBindingFile::empty(),
            recovery_ledger: RecoveryLedgerFile::empty(),
        }
    );
    assert_eq!(
        std::fs::read(home_path.join(HomeMarker::FILE_NAME)).expect("marker after writer block"),
        marker_before
    );
    assert_eq!(
        std::fs::read(home_path.join(CATALOG_FILE_NAME)).expect("Catalog after writer block"),
        catalog_before
    );
    child.kill().expect("stop WAL writer");
    let _ = child.wait();

    let plan = service.prepare(&home_path).expect("prepare Recovery Plan");

    let snapshot = service
        .confirm(&plan.plan_token)
        .expect("confirm Existing Home Recovery");

    assert!(
        matches!(snapshot, BootstrapSnapshot::Bound { ref home_id, .. } if home_id == &home.home_id)
    );
    let files = app_state.load().expect("read rebuilt locator");
    let current = files.binding.current.expect("recovered current binding");
    assert_eq!(current.home_id, home.home_id);
    assert_eq!(
        current.path,
        home_path.canonicalize().expect("canonical Home")
    );
    assert_eq!(current.volume_fsid, volume.fsid);
    assert_eq!(current.volume_uuid, volume.uuid);
    assert_eq!(current.bound_at, CREATED_AT);
    assert!(files.binding.abandoned.is_empty());
    assert!(files.recovery_ledger.active.is_none());
    assert_eq!(
        std::fs::read(home_path.join(HomeMarker::FILE_NAME)).expect("marker after confirmation"),
        marker_before
    );
    assert_eq!(
        std::fs::read(home_path.join(CATALOG_FILE_NAME)).expect("Catalog after confirmation"),
        catalog_before
    );

    // A failure after the CAS never rolls the locator back. On the next
    // bootstrap inspection a replaced or missing Home converges to the
    // ordinary closed state, leaving product writes unavailable.
    let replaced_home_path = dir.path().join("Replaced Home");
    std::fs::rename(&home_path, &replaced_home_path).expect("replace Home after commit");
    assert!(matches!(
        bootstrap.inspect(),
        BootstrapSnapshot::HomeUnavailable { ref home_id, .. } if home_id == &home.home_id
    ));
    assert_eq!(
        app_state
            .load()
            .expect("post-commit locator remains durable")
            .binding
            .current
            .expect("binding remains after closed-state convergence")
            .home_id,
        home.home_id
    );
}
