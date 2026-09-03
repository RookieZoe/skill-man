//! Home Lifecycle flow integration tests (spec §5.5, §10.1; ADR-0012 §5–§6):
//! the real adapters — AppStateStore, Catalog probe, FileSystem, volume
//! identity, prepared Catalog factory, fixture classifier and the bootstrap
//! authority — driven end to end against isolated temp Homes. Volume loss /
//! Reconnect, Restore of a same-identity content failure and the
//! high-friction Abandon with the locator CAS are proven, including the
//! "old volume returns" Abandoned route and a brand-new UUID on the next
//! binding.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::locale_store::LocaleStoreFileSystem;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::{SqliteCatalogStore, SqlitePreparedCatalogFactory};
use skill_man_lib::core::bootstrap::{BootstrapConfig, BootstrapService, BootstrapSnapshot};
use skill_man_lib::core::fixture_recovery::{
    FixtureRecoveryService, RestoreEligibility, RestoreReason, SystemFixtureClassifier,
};
use skill_man_lib::core::home::{BoundHome, HomeId, HomeMarker, VolumeIdentity};
use skill_man_lib::core::home_binding::{
    CandidateMode, HOME_CANDIDATE_KIND, HomeBindingConfig, HomeBindingService,
};
use skill_man_lib::core::home_lifecycle::{HomeLifecycleError, HomeLifecycleService};
use skill_man_lib::seams::app_state_store::{
    AppStateStore, HomeBindingFile, RecoveryLedgerFile, RecoveryOperationRecord,
};
use skill_man_lib::seams::catalog_probe::CURRENT_CATALOG_SCHEMA_VERSION;
use skill_man_lib::seams::catalog_probe::CatalogProbe;
use skill_man_lib::seams::locale_store::{LocaleSelection, LocaleStore};
use skill_man_lib::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

use common::{CATALOG_FILE_NAME, HOME_ID, LIBRARY_ROOT_NAME, STATE_DIR_NAME};

const TEST_FSID: &str = "test-fsid";
const TEST_UUID: &str = "test-uuid";

struct ToggleVolumeIdentitySource {
    volume: Mutex<Option<VolumeIdentity>>,
}

impl ToggleVolumeIdentitySource {
    fn new(volume: Option<VolumeIdentity>) -> Self {
        Self {
            volume: Mutex::new(volume),
        }
    }

    fn set(&self, volume: Option<VolumeIdentity>) {
        *self.volume.lock().unwrap() = volume;
    }
}

impl VolumeIdentitySource for ToggleVolumeIdentitySource {
    fn volume_identity(&self, _path: &Path) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
        Ok(self.volume.lock().unwrap().clone())
    }
}

struct Composition {
    _dir: tempfile::TempDir,
    root: PathBuf,
    state_dir: PathBuf,
    default_home: PathBuf,
    home: BoundHome,
    volume: Arc<ToggleVolumeIdentitySource>,
    bootstrap: Arc<BootstrapService>,
    recovery: Arc<FixtureRecoveryService>,
    lifecycle: Arc<HomeLifecycleService>,
    binding: Arc<HomeBindingService>,
}

impl Composition {
    fn home_path(&self) -> &Path {
        &self.home.path
    }

    fn catalog_path(&self) -> PathBuf {
        self.home.path.join(CATALOG_FILE_NAME)
    }

    fn locator(&self) -> skill_man_lib::seams::app_state_store::HomeBindingFile {
        AppStateStoreFileSystem::new(self.state_dir.clone())
            .load()
            .expect("app state")
            .binding
    }

    fn marker(&self) -> HomeMarker {
        let content =
            std::fs::read_to_string(self.home.path.join(HomeMarker::FILE_NAME)).expect("marker");
        HomeMarker::parse(&content).expect("valid marker")
    }

    /// Run a SQL batch against the Catalog.
    fn with_sql(&self, operation: &str, f: impl FnOnce(&Connection)) {
        let connection = Connection::open(self.catalog_path()).expect(operation);
        f(&connection);
        drop(connection);
    }
}

fn compose(home_path: Option<&Path>) -> Composition {
    let dir = tempfile::tempdir().expect("temp dir");
    // tempfile paths under /var are symlinks to /private/var; the product
    // normalizes candidate paths, so the test roots must be canonical.
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let state_dir = root.join(STATE_DIR_NAME);
    let default_home = root.join(LIBRARY_ROOT_NAME);
    std::fs::create_dir_all(&state_dir).expect("state dir");

    let home_path = home_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_home.clone());
    let home = BoundHome {
        home_id: HomeId(HOME_ID.into()),
        path: home_path,
        volume_fsid: TEST_FSID.into(),
        volume_uuid: TEST_UUID.into(),
        bound_at: "2026-08-01T00:00:00Z".into(),
    };
    std::fs::create_dir_all(&home.path).expect("home dir");
    std::fs::write(
        home.path.join(HomeMarker::FILE_NAME),
        serde_json::to_string_pretty(&HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: home.home_id.clone(),
            volume_fsid: home.volume_fsid.clone(),
            volume_uuid: home.volume_uuid.clone(),
            created_at: home.bound_at.clone(),
        })
        .expect("marker JSON"),
    )
    .expect("marker");
    let _sqlite = SqliteCatalogStore::create_bound(&home, &home.path.join(CATALOG_FILE_NAME))
        .expect("create bound Catalog");
    let app_state_store = AppStateStoreFileSystem::new(state_dir.clone());
    app_state_store
        .write_locator(&skill_man_lib::seams::app_state_store::HomeBindingFile {
            schema_version: 1,
            current: Some(skill_man_lib::seams::app_state_store::HomeBindingRecord {
                home_id: home.home_id.clone(),
                path: home.path.clone(),
                volume_fsid: home.volume_fsid.clone(),
                volume_uuid: home.volume_uuid.clone(),
                bound_at: home.bound_at.clone(),
            }),
            abandoned: vec![],
        })
        .expect("locator");

    let filesystem = Arc::new(MacOsFileSystem::new(root.clone()));
    let app_state = Arc::new(app_state_store);
    let probe = Arc::new(SqliteCatalogProbe::new());
    let classifier = Arc::new(SystemFixtureClassifier::new(
        probe.clone(),
        filesystem.clone(),
        CATALOG_FILE_NAME.into(),
    ));
    let bootstrap_config = BootstrapConfig {
        state_dir: state_dir.clone(),
        default_home_path: default_home.clone(),
        catalog_file_name: CATALOG_FILE_NAME.into(),
    };
    let volume = Arc::new(ToggleVolumeIdentitySource::new(Some(VolumeIdentity {
        fsid: TEST_FSID.into(),
        uuid: TEST_UUID.into(),
    })));
    let bootstrap = Arc::new(BootstrapService::new(
        app_state.clone(),
        volume.clone(),
        probe.clone(),
        filesystem.clone(),
        classifier,
        bootstrap_config,
    ));
    let recovery = Arc::new(FixtureRecoveryService::new(
        app_state.clone(),
        probe.clone(),
        filesystem.clone(),
        Arc::new(SqlitePreparedCatalogFactory),
        bootstrap.clone(),
        BootstrapConfig {
            state_dir: state_dir.clone(),
            default_home_path: default_home.clone(),
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    ));
    let lifecycle = Arc::new(HomeLifecycleService::new(
        app_state.clone(),
        bootstrap.clone(),
    ));
    let binding = Arc::new(HomeBindingService::new(
        app_state,
        volume.clone(),
        probe,
        filesystem,
        Arc::new(SystemFixtureClassifier::new(
            Arc::new(SqliteCatalogProbe::new()),
            Arc::new(MacOsFileSystem::new(root.clone())),
            CATALOG_FILE_NAME.into(),
        )),
        Arc::new(skill_man_lib::adapters::sqlite::SqliteLegacyCatalogMigrator),
        Arc::new(SqlitePreparedCatalogFactory),
        bootstrap.clone(),
        HomeBindingConfig {
            state_dir: state_dir.clone(),
            default_home_path: default_home.clone(),
            catalog_file_name: CATALOG_FILE_NAME.into(),
            agent_skill_dirs: vec![root.join("agents/skills")],
        },
    ));
    Composition {
        _dir: dir,
        root,
        state_dir,
        default_home,
        home,
        volume,
        bootstrap,
        recovery,
        lifecycle,
        binding,
    }
}

fn volume() -> VolumeIdentity {
    VolumeIdentity {
        fsid: TEST_FSID.into(),
        uuid: TEST_UUID.into(),
    }
}

fn assert_bound(snapshot: &BootstrapSnapshot, expected_home_id: &str) {
    match snapshot {
        BootstrapSnapshot::Bound { home_id, .. } => {
            assert_eq!(home_id.0, expected_home_id);
        }
        other => panic!("expected Bound, got {other:?}"),
    }
}

// -- Reconnect Same Home ----------------------------------------------------

#[test]
fn reconnect_restores_the_same_home_id_after_volume_loss() {
    let composition = compose(None);
    let before = composition.locator();

    // Volume offline: HomeUnavailable; reconnect has zero side effects.
    composition.volume.set(None);
    match composition.bootstrap.inspect() {
        BootstrapSnapshot::HomeUnavailable { home_id, .. } => {
            assert_eq!(home_id.0, HOME_ID);
        }
        other => panic!("expected HomeUnavailable, got {other:?}"),
    }
    match composition
        .lifecycle
        .reconnect_same_home()
        .expect("reconnect")
    {
        BootstrapSnapshot::HomeUnavailable { .. } => {}
        other => panic!("expected HomeUnavailable, got {other:?}"),
    }
    assert_eq!(composition.locator(), before, "no locator side effects");

    // Volume back with the same identity: the same home_id is Bound.
    composition.volume.set(Some(volume()));
    assert_bound(
        &composition
            .lifecycle
            .reconnect_same_home()
            .expect("reconnect"),
        HOME_ID,
    );
    assert_eq!(
        composition.locator(),
        before,
        "still no locator side effects"
    );
    assert_eq!(composition.marker().home_id.0, HOME_ID);
}

#[test]
fn reconnect_never_rebinds_a_mismatched_volume() {
    let composition = compose(None);
    let before = composition.locator();

    // The volume comes back with a DIFFERENT identity: still Mismatch, and
    // the locator is never rewritten.
    composition.volume.set(Some(VolumeIdentity {
        fsid: TEST_FSID.into(),
        uuid: "other-uuid".into(),
    }));
    match composition
        .lifecycle
        .reconnect_same_home()
        .expect("reconnect")
    {
        BootstrapSnapshot::HomeIdentityMismatch { home_id, .. } => {
            assert_eq!(home_id.0, HOME_ID);
        }
        other => panic!("expected HomeIdentityMismatch, got {other:?}"),
    }
    assert_eq!(composition.locator(), before);

    // The original volume returns: the same home_id reconnects.
    composition.volume.set(Some(volume()));
    assert_bound(
        &composition
            .lifecycle
            .reconnect_same_home()
            .expect("reconnect"),
        HOME_ID,
    );
}

#[test]
fn candidate_recovery_keeps_the_catalog_fsid_after_a_same_uuid_restart() {
    let composition = compose(None);
    let candidate_path = composition.root.join("candidate-after-crash");
    let candidate = BoundHome {
        home_id: HomeId("a1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
        path: candidate_path.clone(),
        volume_fsid: TEST_FSID.into(),
        volume_uuid: TEST_UUID.into(),
        bound_at: "2026-08-01T00:00:00Z".into(),
    };
    std::fs::create_dir_all(&candidate_path).expect("candidate root");
    for directory in ["skills", "remotes", "operations", "cache", "staging"] {
        std::fs::create_dir_all(candidate_path.join(directory)).expect("candidate layout");
    }
    std::fs::write(
        candidate_path.join(HomeMarker::FILE_NAME),
        serde_json::to_string_pretty(&HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: candidate.home_id.clone(),
            volume_fsid: candidate.volume_fsid.clone(),
            volume_uuid: candidate.volume_uuid.clone(),
            created_at: candidate.bound_at.clone(),
        })
        .expect("candidate marker JSON"),
    )
    .expect("candidate marker");
    let _catalog =
        SqliteCatalogStore::create_bound(&candidate, &candidate_path.join(CATALOG_FILE_NAME))
            .expect("candidate Catalog");

    let store = AppStateStoreFileSystem::new(composition.state_dir.clone());
    store
        .write_locator(&HomeBindingFile {
            schema_version: 1,
            current: None,
            abandoned: vec![],
        })
        .expect("unconfigured locator");
    store
        .write_recovery_ledger(&RecoveryLedgerFile {
            schema_version: 1,
            active: Some(RecoveryOperationRecord {
                operation_id: "candidate-after-crash".into(),
                kind: HOME_CANDIDATE_KIND.into(),
                home_id: Some(candidate.home_id.clone()),
                live_path: Some(candidate_path.clone()),
                prepared_path: None,
                snapshot_path: None,
                manifest_hash: None,
                external_probe: None,
                cursor: Some("verified".into()),
                commit_point: None,
                created_at: candidate.bound_at.clone(),
            }),
            completed: vec![],
        })
        .expect("candidate recovery ledger");

    // APFS can assign a new mount-scoped fsid after a restart while the
    // persistent volume UUID remains unchanged.
    composition.volume.set(Some(VolumeIdentity {
        fsid: "fsid-after-restart".into(),
        uuid: TEST_UUID.into(),
    }));
    let snapshot = composition
        .binding
        .continue_candidate("candidate-after-crash")
        .expect("continue candidate");
    assert_bound(&snapshot, &candidate.home_id.0);

    let locator = composition.locator();
    let current = locator.current.expect("committed locator");
    assert_eq!(current.volume_fsid, TEST_FSID);
    assert_eq!(current.volume_uuid, TEST_UUID);
    SqliteCatalogStore::open_bound(
        &BoundHome {
            home_id: current.home_id,
            path: current.path,
            volume_fsid: current.volume_fsid,
            volume_uuid: current.volume_uuid,
            bound_at: current.bound_at,
        },
        &candidate_path.join(CATALOG_FILE_NAME),
    )
    .expect("the recovered binding reopens the Catalog writable");
}

// -- Restore Bound Home -----------------------------------------------------

/// Break the Catalog's foreign-key closure: an activation referencing a
/// missing Skill makes `PRAGMA foreign_key_check` report a violation.
fn corrupt_catalog(composition: &Composition) {
    composition.with_sql("corrupt Catalog", |connection| {
        // rusqlite enforces foreign keys by default; the violation must
        // be written so `PRAGMA foreign_key_check` can report it.
        connection
            .pragma_update(None, "foreign_keys", false)
            .expect("disable FK enforcement for the corruption");
        connection
            .execute(
                "INSERT INTO activations (
                        skill_id, target_root_id, directory_identity_key, desired_enabled,
                        expected_entry_path, expected_target_path, observed_state,
                        last_enabled_at, last_checked_at
                     ) VALUES (
                        'ghost-skill', 'ghost-root', 'ghost-skill', 1, '/tmp/ghost-entry',
                        '/tmp/ghost-target', 'missing', NULL, NULL
                     )",
                [],
            )
            .expect("insert FK-violating activation");
    });
}

#[test]
fn restore_of_an_integrity_failed_home_promotes_the_same_identity() {
    let composition = compose(None);
    let before = composition.locator();
    // Real user content: a seeded Skill row + entity.
    std::fs::create_dir_all(composition.home_path().join("skills/user-skill")).expect("entity");
    std::fs::write(
        composition.home_path().join("skills/user-skill/SKILL.md"),
        "# User skill\n",
    )
    .expect("entity document");
    composition.with_sql("seed Skill", |connection| {
        connection
            .execute(
                "INSERT INTO skills (
                        id, directory_name, directory_identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path, health,
                        created_at, updated_at
                     ) VALUES (
                        'user-skill', 'user-skill', 'user-skill', 'User skill',
                        'Real content.', 'remote_install', '/home/skills/user-skill',
                        '/home/skills/user-skill', 'healthy', '2026-08-01T00:00:00Z',
                        '2026-08-01T00:00:00Z'
                     )",
                [],
            )
            .expect("seed Skill row");
    });

    // Content validation fails: Bound read-only with IntegrityFailed.
    corrupt_catalog(&composition);
    match composition.bootstrap.inspect() {
        BootstrapSnapshot::Bound {
            catalog_access:
                skill_man_lib::core::bootstrap::CatalogAccess::ReadOnly {
                    reason: skill_man_lib::core::write_gate::ReadOnlyReason::IntegrityFailed,
                },
            home_id,
            ..
        } => {
            assert_eq!(home_id.0, HOME_ID);
        }
        other => panic!("expected Bound read-only IntegrityFailed, got {other:?}"),
    }

    // Eligibility: same identity proven, content failed.
    match composition.recovery.restore_eligibility().expect("probe") {
        RestoreEligibility::RestoreRequired {
            home_id,
            path,
            reason,
        } => {
            assert_eq!(home_id.0, HOME_ID);
            assert_eq!(path, composition.home_path());
            assert_eq!(reason, RestoreReason::CatalogIntegrityFailed);
        }
        other => panic!("expected RestoreRequired, got {other:?}"),
    }

    // Plan → apply → commit: the full recovery state machine.
    let plan = composition.recovery.plan_restore().expect("plan restore");
    let result = composition
        .recovery
        .apply(&plan.plan_token)
        .expect("apply restore");
    assert!(result.awaiting_commit, "the clean Home awaits commit");
    let snapshot = composition
        .recovery
        .confirm_result(&result.operation_id)
        .expect("confirm restore");
    assert_bound(&snapshot, HOME_ID);

    // Locator identity unchanged; marker unchanged; fresh clean Catalog with
    // the SAME home_id and zero user rows (the old content lives in the
    // Safety Snapshot, which is never auto-deleted).
    assert_eq!(
        composition.locator(),
        before,
        "the locator identity never changes"
    );
    assert_eq!(composition.marker().home_id.0, HOME_ID);
    let report = SqliteCatalogProbe::new()
        .probe(&composition.catalog_path())
        .expect("probe restored Catalog");
    assert_eq!(report.schema_version, Some(CURRENT_CATALOG_SCHEMA_VERSION));
    assert!(report.integrity_ok && report.foreign_keys_ok);
    assert_eq!(
        report
            .home_identity
            .as_ref()
            .map(|identity| identity.home_id.0.as_str()),
        Some(HOME_ID),
        "the restored Catalog carries the same home_id"
    );
    let snapshots = composition.recovery.list_snapshots().expect("snapshots");
    assert_eq!(snapshots.len(), 1, "the Safety Snapshot is kept");
    assert!(snapshots[0].path.is_dir());
    assert!(
        snapshots[0]
            .path
            .join("skills/user-skill/SKILL.md")
            .is_file(),
        "the old user content is preserved in the snapshot"
    );
}

#[test]
fn restore_is_refused_for_a_healthy_or_mismatched_home() {
    // Healthy: nothing to restore.
    let composition = compose(None);
    assert_eq!(
        composition.recovery.restore_eligibility().expect("probe"),
        RestoreEligibility::NotRequired
    );
    assert!(matches!(
        composition.recovery.plan_restore(),
        Err(skill_man_lib::core::fixture_recovery::FixtureRecoveryError::NotRestorable(_))
    ));

    // Mismatched volume: Restore never applies (Abandon is the only exit).
    composition.volume.set(Some(VolumeIdentity {
        fsid: TEST_FSID.into(),
        uuid: "other-uuid".into(),
    }));
    assert!(matches!(
        composition.recovery.restore_eligibility().expect("probe"),
        RestoreEligibility::NotApplicable { .. }
    ));
}

#[test]
fn restore_resumes_after_a_crash_mid_operation() {
    let composition = compose(None);
    corrupt_catalog(&composition);
    let plan = composition.recovery.plan_restore().expect("plan restore");
    let operation_id = plan.plan_token.clone();

    // Crash right after the Safety Snapshot rename: the ledger cursor is
    // `snapshotted` and the live Home sits at the sibling snapshot path.
    let parent = composition
        .home_path()
        .parent()
        .expect("home parent")
        .to_path_buf();
    let home_name = composition
        .home_path()
        .file_name()
        .expect("home name")
        .to_string_lossy()
        .into_owned();
    let snapshot_path = parent.join(format!("{home_name}.snapshot-{operation_id}"));
    std::fs::rename(composition.home_path(), &snapshot_path).expect("snapshot rename");
    let store = AppStateStoreFileSystem::new(composition.state_dir.clone());
    let mut files = store.load().expect("app state");
    let active = files.recovery_ledger.active.as_mut().expect("active op");
    assert_eq!(active.operation_id, operation_id);
    active.cursor = Some("snapshotted".into());
    active.snapshot_path = Some(snapshot_path.clone());
    store
        .write_recovery_ledger(&files.recovery_ledger)
        .expect("crash ledger");

    // A fresh service instance (a restart) converges deterministically from
    // the recorded cursor: prepare → validate → promote → verify.
    let restarted = Arc::new(FixtureRecoveryService::new(
        Arc::new(store),
        Arc::new(SqliteCatalogProbe::new()),
        Arc::new(MacOsFileSystem::new(composition.root.clone())),
        Arc::new(SqlitePreparedCatalogFactory),
        composition.bootstrap.clone(),
        BootstrapConfig {
            state_dir: composition.state_dir.clone(),
            default_home_path: composition.default_home.clone(),
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    ));
    let result = restarted.apply(&operation_id).expect("resume restore");
    assert!(
        result.awaiting_commit,
        "the crash converges to AwaitingCommit"
    );
    let snapshot = restarted
        .confirm_result(&result.operation_id)
        .expect("confirm restore");
    assert_bound(&snapshot, HOME_ID);
    assert!(
        composition.home_path().is_dir(),
        "the promoted clean Home is live again"
    );
    assert!(
        snapshot_path.is_dir(),
        "the Safety Snapshot is never auto-deleted"
    );
    assert_eq!(
        composition
            .locator()
            .current
            .as_ref()
            .map(|c| c.home_id.0.as_str()),
        Some(HOME_ID)
    );
}

// -- Abandon Home and Start New ---------------------------------------------

#[test]
fn abandon_requires_the_typed_home_id_and_commits_via_locator_cas() {
    // The abandoned Home sits at the default path: after the CAS the route
    // is Abandoned, never an orphaned candidate.
    let composition = compose(None);
    std::fs::write(composition.home_path().join("user-data.txt"), "keep me").expect("user file");
    let locale = LocaleStoreFileSystem::new(composition.state_dir.clone());
    locale
        .store_selection(LocaleSelection::ZhHans)
        .expect("persist locale");

    let preview = composition.lifecycle.plan_abandon().expect("plan");
    assert_eq!(preview.home_id.0, HOME_ID);
    assert_eq!(preview.path, composition.home_path());

    // Wrong typed confirmation: refused, zero side effects.
    let wrong = HomeId("00000000-0000-4000-8000-000000000000".into());
    assert!(matches!(
        composition
            .lifecycle
            .apply_abandon(&preview.plan_token, &wrong),
        Err(HomeLifecycleError::ConfirmationMismatch)
    ));
    assert_eq!(
        composition
            .locator()
            .current
            .as_ref()
            .map(|c| c.home_id.0.as_str()),
        Some(HOME_ID)
    );

    // Correct confirmation: the locator CAS is the commit point.
    let snapshot = composition
        .lifecycle
        .apply_abandon(&preview.plan_token, &HomeId(HOME_ID.into()))
        .expect("apply abandon");
    assert!(
        matches!(snapshot, BootstrapSnapshot::Abandoned { .. }),
        "the abandoned site is the Abandoned route, got {snapshot:?}"
    );
    let locator = composition.locator();
    assert!(locator.current.is_none(), "no current binding remains");
    assert_eq!(locator.abandoned.len(), 1);
    assert_eq!(locator.abandoned[0].home_id.0, HOME_ID);
    assert_eq!(locator.abandoned[0].path, composition.home_path());

    // The old Home, its content and the locale are untouched.
    assert!(composition.home_path().join("user-data.txt").is_file());
    assert!(composition.catalog_path().is_file());
    assert_eq!(
        locale.load_selection().expect("locale").unwrap(),
        LocaleSelection::ZhHans,
        "Abandon never changes the locale"
    );

    // The CAS race: a second plan+apply can never double-record.
    let racer = HomeLifecycleService::new(
        Arc::new(AppStateStoreFileSystem::new(composition.state_dir.clone())),
        composition.bootstrap.clone(),
    );
    assert!(matches!(
        racer.plan_abandon(),
        Err(HomeLifecycleError::NotAbandonable(_))
    ));
    assert_eq!(composition.locator().abandoned.len(), 1);
}

#[test]
fn a_new_binding_after_abandon_gets_a_brand_new_home_id() {
    let composition = compose(None);
    composition
        .lifecycle
        .apply_abandon(
            &composition
                .lifecycle
                .plan_abandon()
                .expect("plan")
                .plan_token,
            &HomeId(HOME_ID.into()),
        )
        .expect("abandon");

    // The abandoned default path is never a candidate: fresh validation at
    // the default path fails closed (non-empty), and the wizard continues
    // via Choose….
    let abandoned = composition
        .binding
        .prepare_home(&composition.default_home)
        .expect_err("the abandoned site is never a candidate");
    assert!(matches!(
        abandoned,
        skill_man_lib::core::home_binding::HomeBindingError::CandidateInvalid {
            reason: skill_man_lib::core::home_binding::CandidateInvalidReason::NotEmpty,
            ..
        }
    ));

    // A fresh candidate elsewhere binds with a brand-new UUID.
    let new_path = composition.root.join("new-home");
    let candidate = composition
        .binding
        .prepare_home(&new_path)
        .expect("prepare new home");
    assert_eq!(candidate.mode, CandidateMode::Fresh);
    let snapshot = composition
        .binding
        .confirm_home(&candidate.token)
        .expect("confirm new home");
    let new_home_id = match &snapshot {
        BootstrapSnapshot::Bound { home_id, .. } => home_id.0.clone(),
        other => panic!("expected Bound, got {other:?}"),
    };
    assert_ne!(new_home_id, HOME_ID, "the new binding uses a new UUID");

    let locator = composition.locator();
    let current = locator.current.expect("new binding");
    assert_eq!(current.home_id.0, new_home_id);
    assert_eq!(locator.abandoned.len(), 1, "the old id stays in history");
}
