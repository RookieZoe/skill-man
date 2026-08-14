//! Home Binding flow integration tests (spec §5.3, §5.4, §10.1): the real
//! adapters — AppStateStore, Catalog probe/migration, FileSystem, volume
//! identity, prepared Catalog factory, fixture classifier and the bootstrap
//! authority — driven end to end against isolated temp Homes. Fresh
//! candidates, Legacy in-place (zero move) and Legacy copy transitions are
//! proven, plus crash convergence at every durable cursor.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::{
    SqliteCatalogStore, SqliteLegacyCatalogMigrator, SqlitePreparedCatalogFactory,
};
use skill_man_lib::core::bootstrap::{BootstrapConfig, BootstrapService, BootstrapSnapshot};
use skill_man_lib::core::fixture_recovery::SystemFixtureClassifier;
use skill_man_lib::core::home::{BoundHome, HomeId, HomeMarker, VolumeIdentity};
use skill_man_lib::core::home_binding::{
    CandidateInvalidReason, CandidateMode, HomeBindingConfig, HomeBindingError, HomeBindingService,
};
use skill_man_lib::seams::app_state_store::{
    AppStateStore, HomeBindingFile, RecoveryLedgerFile, RecoveryOperationRecord,
};
use skill_man_lib::seams::catalog_probe::CatalogProbe;
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

use common::{CATALOG_FILE_NAME, LIBRARY_ROOT_NAME, STATE_DIR_NAME};

const TEST_FSID: &str = "test-fsid";
const TEST_UUID: &str = "test-uuid";

struct FixedVolumeIdentity(Option<VolumeIdentity>);

impl VolumeIdentitySource for FixedVolumeIdentity {
    fn volume_identity(&self, _path: &Path) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
        Ok(self.0.clone())
    }
}

struct Composition {
    _dir: tempfile::TempDir,
    root: PathBuf,
    state_dir: PathBuf,
    default_home: PathBuf,
    agent_dir: PathBuf,
    bootstrap: Arc<BootstrapService>,
    binding: Arc<HomeBindingService>,
    filesystem: Arc<MacOsFileSystem>,
}

impl Composition {
    fn home_root(&self) -> PathBuf {
        self.root.clone()
    }
}

fn compose(volume: Option<VolumeIdentity>) -> Composition {
    let dir = tempfile::tempdir().expect("temp dir");
    // tempfile paths under /var are symlinks to /private/var; the product
    // normalizes every candidate path, so the test roots must be canonical
    // from the start or path comparisons drift.
    let root = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let state_dir = root.join(STATE_DIR_NAME);
    let default_home = root.join(LIBRARY_ROOT_NAME);
    let agent_dir = root.join("agents/skills");
    std::fs::create_dir_all(&state_dir).expect("state dir");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");

    let filesystem = Arc::new(MacOsFileSystem::new(root.clone()));
    let app_state = Arc::new(AppStateStoreFileSystem::new(state_dir.clone()));
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
    let bootstrap = Arc::new(BootstrapService::new(
        app_state.clone(),
        Arc::new(FixedVolumeIdentity(volume.clone())),
        probe.clone(),
        filesystem.clone(),
        classifier,
        bootstrap_config,
    ));
    let binding = Arc::new(HomeBindingService::new(
        app_state,
        Arc::new(FixedVolumeIdentity(volume)),
        probe,
        filesystem.clone(),
        Arc::new(SystemFixtureClassifier::new(
            Arc::new(SqliteCatalogProbe::new()),
            filesystem.clone(),
            CATALOG_FILE_NAME.into(),
        )),
        Arc::new(SqliteLegacyCatalogMigrator),
        Arc::new(SqlitePreparedCatalogFactory),
        bootstrap.clone(),
        HomeBindingConfig {
            state_dir: state_dir.clone(),
            default_home_path: default_home.clone(),
            catalog_file_name: CATALOG_FILE_NAME.into(),
            agent_skill_dirs: vec![agent_dir.clone()],
        },
    ));
    Composition {
        _dir: dir,
        root,
        state_dir,
        default_home,
        agent_dir,
        bootstrap,
        binding,
        filesystem,
    }
}

fn volume() -> VolumeIdentity {
    VolumeIdentity {
        fsid: TEST_FSID.into(),
        uuid: TEST_UUID.into(),
    }
}

/// A real Legacy Home: a complete v4 Catalog with user data rows, plus a
/// user Skill entity directory — everything a migrated Home must preserve.
fn seed_legacy_home(home_root: &Path) {
    std::fs::create_dir_all(home_root.join("skills/user-skill")).expect("entity dir");
    std::fs::write(
        home_root.join("skills/user-skill/SKILL.md"),
        "# User skill\n\nReal legacy content.\n",
    )
    .expect("entity document");
    let path = home_root.join(CATALOG_FILE_NAME);
    // Build the current schema through the production factory, then drop the
    // identity columns and rewind the version: an exact pre-identity v4 file.
    let fresh = SqliteCatalogStore::open(&path).expect("fresh v5 Catalog");
    drop(fresh);
    let connection = Connection::open(&path).expect("open Catalog");
    for column in ["home_id", "volume_fsid", "volume_uuid", "home_bound_at"] {
        connection
            .execute(
                &format!("ALTER TABLE catalog_meta DROP COLUMN {column}"),
                [],
            )
            .expect("drop identity column");
    }
    // The current schema also carries the Remote Source Parent tables; a
    // real pre-identity v4 file has none of them.
    for table in [
        "remote_source_parents",
        "remote_source_aliases",
        "remote_bindings",
    ] {
        connection
            .execute(&format!("DROP TABLE {table}"), [])
            .expect("drop v6 parent table");
    }
    // A real v4 file still carries the legacy remote_sources table the v6
    // migration consumes.
    connection
        .execute(
            "CREATE TABLE remote_sources (
                skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                source_url TEXT NOT NULL,
                requested_ref TEXT NOT NULL,
                resolved_commit TEXT NOT NULL,
                skill_path TEXT NOT NULL,
                last_checked_at INTEGER,
                last_updated_at INTEGER
             )",
            [],
        )
        .expect("create legacy remote_sources table");
    connection
        .execute(
            "UPDATE catalog_meta SET schema_version = 4, first_run_completed_at = \
             '2026-07-01T00:00:00Z' WHERE singleton = 1",
            [],
        )
        .expect("rewind schema");
    connection
        .execute(
            "INSERT INTO skills (
                id, directory_name, identity_key, display_name, description,
                source_kind, library_entry_path, final_entity_path, health,
                created_at, updated_at
             ) VALUES (
                'user-skill', 'user-skill', 'user-skill', 'User skill',
                'Real legacy content.', 'file_install', '/old/skills/user-skill',
                '/old/skills/user-skill', 'healthy', '2026-07-01T00:00:00Z',
                '2026-07-01T00:00:00Z'
             )",
            [],
        )
        .expect("seed user Skill row");
    connection
        .execute(
            "INSERT INTO agents (
                id, name, kind, skills_path, path_identity_key, detected,
                compatibility, created_at, updated_at
             ) VALUES (
                'claude-code', 'Claude Code', 'claude_preset', '~/.claude/skills',
                '~/.claude/skills', 1, 'verified', '1970-01-01T00:00:00Z',
                '1970-01-01T00:00:00Z'
             )",
            [],
        )
        .expect("seed user Agent row");
    drop(connection);
}

fn is_bound(snapshot: &BootstrapSnapshot) -> bool {
    matches!(snapshot, BootstrapSnapshot::Bound { .. })
}

fn assert_bound_home(composition: &Composition, expected_path: &Path) {
    let snapshot = composition.bootstrap.inspect();
    match snapshot {
        BootstrapSnapshot::Bound { .. } => {}
        other => panic!("expected Bound, got {other:?}"),
    }
    let files = AppStateStoreFileSystem::new(composition.state_dir.clone())
        .load()
        .expect("app state");
    let current = files.binding.current.expect("current binding");
    assert_eq!(current.path, expected_path);
    // The binding identity must match the marker and the Catalog.
    let marker =
        std::fs::read_to_string(expected_path.join(HomeMarker::FILE_NAME)).expect("marker file");
    let marker = HomeMarker::parse(&marker).expect("valid marker");
    assert_eq!(marker.home_id, current.home_id);
    let report = SqliteCatalogProbe::new()
        .probe(&expected_path.join(CATALOG_FILE_NAME))
        .expect("probe Catalog");
    assert_eq!(report.schema_version, Some(6));
    let identity = report.home_identity.expect("Catalog identity");
    assert_eq!(identity.home_id, current.home_id);
    assert_eq!(identity.volume_fsid, current.volume_fsid);
    assert_eq!(identity.volume_uuid, current.volume_uuid);
}

fn ledger(composition: &Composition) -> RecoveryLedgerFile {
    AppStateStoreFileSystem::new(composition.state_dir.clone())
        .load()
        .expect("app state")
        .recovery_ledger
}

fn write_ledger(composition: &Composition, ledger: &RecoveryLedgerFile) {
    AppStateStoreFileSystem::new(composition.state_dir.clone())
        .write_recovery_ledger(ledger)
        .expect("write ledger");
}

/// Simulate a crash with the given operation and cursor state plus whatever
/// artifacts the scenario needs.
fn crash_operation(
    _composition: &Composition,
    operation_id: &str,
    kind: &str,
    home_id: &str,
    live_path: &Path,
    prepared_path: Option<&Path>,
    cursor: &str,
) -> RecoveryLedgerFile {
    let mut ledger = RecoveryLedgerFile::empty();
    ledger.active = Some(RecoveryOperationRecord {
        operation_id: operation_id.into(),
        kind: kind.into(),
        home_id: Some(HomeId(home_id.into())),
        live_path: Some(live_path.to_path_buf()),
        prepared_path: prepared_path.map(Path::to_path_buf),
        snapshot_path: None,
        manifest_hash: None,
        external_probe: None,
        cursor: Some(cursor.into()),
        commit_point: None,
        created_at: "2026-08-10T00:00:00Z".into(),
    });
    ledger
}

// -- fresh candidate --------------------------------------------------------

#[test]
fn fresh_default_confirm_binds_and_verifies() {
    let composition = compose(Some(volume()));
    let candidate = composition
        .binding
        .prepare_home(&composition.default_home)
        .expect("prepare");
    assert_eq!(candidate.mode, CandidateMode::Fresh);
    assert_eq!(candidate.path, composition.default_home);

    let snapshot = composition
        .binding
        .confirm_home(&candidate.token)
        .expect("confirm");
    assert!(is_bound(&snapshot), "expected Bound, got {snapshot:?}");
    assert_bound_home(&composition, &composition.default_home);

    // Standard layout, marker, v5 Catalog with identity.
    for directory in ["skills", "remotes", "operations", "cache", "staging"] {
        assert!(
            composition
                .filesystem
                .path_is_directory(&composition.default_home.join(directory))
                .expect("layout probe"),
            "missing layout directory {directory}"
        );
    }
    assert!(
        composition
            .default_home
            .join(format!("{CATALOG_FILE_NAME}-wal"))
            .exists()
    );
    // The ledger operation is completed, not active.
    assert!(ledger(&composition).active.is_none());
    assert_eq!(ledger(&composition).completed.len(), 1);

    // A second inspect is still Bound (no re-selection, no re-binding).
    assert!(is_bound(&composition.bootstrap.inspect()));
}

#[test]
fn fresh_candidate_validation_matrix_rejects_unsafe_paths() {
    let composition = compose(Some(volume()));
    // Symlink component.
    let link_dir = composition.home_root().join("linked");
    std::os::unix::fs::symlink(composition.home_root().join("target"), &link_dir).expect("symlink");
    std::fs::create_dir_all(composition.home_root().join("target")).expect("target");
    let error = composition
        .binding
        .prepare_home(&link_dir.join("candidate"))
        .expect_err("symlink component must be rejected");
    assert!(matches!(
        error,
        HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::SymlinkComponent,
            ..
        }
    ));

    // State-directory overlap.
    let error = composition
        .binding
        .prepare_home(&composition.state_dir.join("nested"))
        .expect_err("state overlap must be rejected");
    assert!(matches!(
        error,
        HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::StateDirOverlap,
            ..
        }
    ));
    let error = composition
        .binding
        .prepare_home(&composition.state_dir)
        .expect_err("state equal must be rejected");
    assert!(matches!(
        error,
        HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::StateDirOverlap,
            ..
        }
    ));

    // Agent-skills overlap.
    let error = composition
        .binding
        .prepare_home(&composition.agent_dir)
        .expect_err("agent overlap must be rejected");
    assert!(matches!(
        error,
        HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::AgentDirOverlap,
            ..
        }
    ));

    // Non-empty directory.
    let occupied = composition.home_root().join("occupied");
    std::fs::create_dir_all(occupied.join("user-content")).expect("content");
    let error = composition
        .binding
        .prepare_home(&occupied)
        .expect_err("non-empty must be rejected");
    assert!(matches!(
        error,
        HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::NotEmpty,
            ..
        }
    ));

    // Existing regular file.
    let file_path = composition.home_root().join("a-file");
    std::fs::write(&file_path, "x").expect("file");
    let error = composition
        .binding
        .prepare_home(&file_path)
        .expect_err("file must be rejected");
    assert!(matches!(
        error,
        HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::NotDirectory,
            ..
        }
    ));

    // Missing parent.
    let error = composition
        .binding
        .prepare_home(&composition.home_root().join("no-parent/child"))
        .expect_err("missing parent must be rejected");
    assert!(matches!(
        error,
        HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::ParentMissing,
            ..
        }
    ));

    // No volume identity.
    let no_volume = compose(None);
    let error = no_volume
        .binding
        .prepare_home(&no_volume.default_home)
        .expect_err("no volume identity must be rejected");
    assert!(matches!(
        error,
        HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::NoVolumeIdentity,
            ..
        }
    ));
}

#[test]
fn prepare_leaves_zero_artifacts_when_rejected() {
    let composition = compose(Some(volume()));
    let occupied = composition.home_root().join("occupied");
    std::fs::create_dir_all(&occupied).expect("dir");
    // An empty directory is a reusable candidate (spec §5.3); preparing it
    // writes nothing.
    composition
        .binding
        .prepare_home(&occupied)
        .expect("empty directory is reusable");
    assert_eq!(
        std::fs::read_dir(&occupied).expect("read").count(),
        0,
        "preparing a candidate must not create anything inside"
    );
    // A non-empty directory is rejected and still writes nothing.
    std::fs::write(occupied.join("user-content.txt"), "x").expect("content");
    let _ = composition
        .binding
        .prepare_home(&occupied)
        .expect_err("non-empty must be rejected");
    assert_eq!(
        std::fs::read_dir(&occupied).expect("read").count(),
        1,
        "a rejected candidate must not create anything inside"
    );
    assert!(!composition.default_home.exists());
    assert!(ledger(&composition).active.is_none());
}

#[test]
fn cancel_of_interrupted_fresh_candidate_leaves_zero_artifacts() {
    let composition = compose(Some(volume()));
    composition
        .binding
        .prepare_home(&composition.default_home)
        .expect("prepare");

    // Simulate a crash right after the artifacts were created: the ledger
    // cursor is `created` and the Home exists.
    let home_id = "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
    let ledger_file = crash_operation(
        &composition,
        "hb-crash",
        "home_candidate",
        home_id,
        &composition.default_home,
        None,
        "created",
    );
    write_ledger(&composition, &ledger_file);
    // Recreate the candidate artifacts with the recorded identity.
    let bound = BoundHome {
        home_id: HomeId(home_id.into()),
        path: composition.default_home.clone(),
        volume_fsid: TEST_FSID.into(),
        volume_uuid: TEST_UUID.into(),
        bound_at: "2026-08-10T00:00:00Z".into(),
    };
    std::fs::create_dir_all(&composition.default_home).expect("Home dir");
    for directory in ["skills", "remotes", "operations", "cache", "staging"] {
        std::fs::create_dir_all(composition.default_home.join(directory)).expect("layout");
    }
    let _ =
        SqliteCatalogStore::create_bound(&bound, &composition.default_home.join(CATALOG_FILE_NAME))
            .expect("Catalog");
    std::fs::write(
        composition.default_home.join(HomeMarker::FILE_NAME),
        serde_json::to_string_pretty(&HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: HomeId(home_id.into()),
            volume_fsid: TEST_FSID.into(),
            volume_uuid: TEST_UUID.into(),
            created_at: "2026-08-10T00:00:00Z".into(),
        })
        .expect("marker JSON"),
    )
    .expect("marker");

    let snapshot = composition
        .binding
        .cancel_candidate("hb-crash")
        .expect("cancel");
    assert!(
        matches!(snapshot, BootstrapSnapshot::Unconfigured),
        "expected Unconfigured, got {snapshot:?}"
    );
    assert!(
        !composition.default_home.exists(),
        "the candidate Home must be fully removed"
    );
    assert!(ledger(&composition).active.is_none());
    assert_eq!(ledger(&composition).completed.len(), 1);
}

#[test]
fn cancel_refuses_foreign_content_and_never_guesses() {
    let composition = compose(Some(volume()));
    let home_id = "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
    let ledger_file = crash_operation(
        &composition,
        "hb-crash",
        "home_candidate",
        home_id,
        &composition.default_home,
        None,
        "created",
    );
    write_ledger(&composition, &ledger_file);
    std::fs::create_dir_all(&composition.default_home).expect("Home dir");
    std::fs::write(composition.default_home.join("user-notes.txt"), "not ours")
        .expect("foreign file");

    let error = composition
        .binding
        .cancel_candidate("hb-crash")
        .expect_err("foreign content must block cancellation");
    assert!(matches!(error, HomeBindingError::NotCancellable(_)));
    assert!(
        composition.default_home.join("user-notes.txt").exists(),
        "foreign content must survive"
    );

    // A migrated Legacy Catalog (rows) also blocks cancellation.
    seed_legacy_home(&composition.default_home);
    let ledger_file = crash_operation(
        &composition,
        "hb-crash-2",
        "home_candidate",
        home_id,
        &composition.default_home,
        None,
        "created",
    );
    write_ledger(&composition, &ledger_file);
    let error = composition
        .binding
        .cancel_candidate("hb-crash-2")
        .expect_err("Legacy rows must block cancellation");
    assert!(matches!(error, HomeBindingError::NotCancellable(_)));
    assert!(
        composition.default_home.join(CATALOG_FILE_NAME).exists(),
        "the migrated Catalog must survive"
    );
}

#[test]
fn continue_after_precommit_crash_completes_the_same_binding() {
    let composition = compose(Some(volume()));
    let home_id = "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
    let ledger_file = crash_operation(
        &composition,
        "hb-crash",
        "home_candidate",
        home_id,
        &composition.default_home,
        None,
        "preparing",
    );
    write_ledger(&composition, &ledger_file);
    // Nothing was created yet: Continue must create, verify and commit.
    let snapshot = composition
        .binding
        .continue_candidate("hb-crash")
        .expect("continue");
    assert!(is_bound(&snapshot), "expected Bound, got {snapshot:?}");
    let current = AppStateStoreFileSystem::new(composition.state_dir.clone())
        .load()
        .expect("app state")
        .binding
        .current
        .expect("binding");
    assert_eq!(
        current.home_id.0, home_id,
        "the same identity is rolled forward"
    );
    assert!(ledger(&composition).active.is_none());
}

#[test]
fn continue_after_postcommit_crash_only_rolls_forward() {
    let composition = compose(Some(volume()));
    let home_id = "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
    // The locator was committed but the ledger still says `verified`.
    let binding = HomeBindingFile {
        schema_version: 1,
        current: Some(skill_man_lib::seams::app_state_store::HomeBindingRecord {
            home_id: HomeId(home_id.into()),
            path: composition.default_home.clone(),
            volume_fsid: TEST_FSID.into(),
            volume_uuid: TEST_UUID.into(),
            bound_at: "2026-08-10T00:00:00Z".into(),
        }),
        abandoned: vec![],
    };
    AppStateStoreFileSystem::new(composition.state_dir.clone())
        .write_locator(&binding)
        .expect("locator");
    let ledger_file = crash_operation(
        &composition,
        "hb-crash",
        "home_candidate",
        home_id,
        &composition.default_home,
        None,
        "verified",
    );
    write_ledger(&composition, &ledger_file);
    // Complete artifacts for the same identity.
    let bound = BoundHome {
        home_id: HomeId(home_id.into()),
        path: composition.default_home.clone(),
        volume_fsid: TEST_FSID.into(),
        volume_uuid: TEST_UUID.into(),
        bound_at: "2026-08-10T00:00:00Z".into(),
    };
    std::fs::create_dir_all(&composition.default_home).expect("Home dir");
    for directory in ["skills", "remotes", "operations", "cache", "staging"] {
        std::fs::create_dir_all(composition.default_home.join(directory)).expect("layout");
    }
    let _ =
        SqliteCatalogStore::create_bound(&bound, &composition.default_home.join(CATALOG_FILE_NAME))
            .expect("Catalog");
    std::fs::write(
        composition.default_home.join(HomeMarker::FILE_NAME),
        serde_json::to_string_pretty(&HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: HomeId(home_id.into()),
            volume_fsid: TEST_FSID.into(),
            volume_uuid: TEST_UUID.into(),
            created_at: "2026-08-10T00:00:00Z".into(),
        })
        .expect("marker JSON"),
    )
    .expect("marker");

    let snapshot = composition
        .binding
        .continue_candidate("hb-crash")
        .expect("continue");
    assert!(is_bound(&snapshot), "expected Bound, got {snapshot:?}");
    assert_eq!(
        AppStateStoreFileSystem::new(composition.state_dir.clone())
            .load()
            .expect("app state")
            .binding
            .current
            .expect("binding")
            .home_id
            .0,
        home_id
    );
    assert!(ledger(&composition).active.is_none());
}

// -- Legacy in-place --------------------------------------------------------

#[test]
fn legacy_default_binds_in_place_with_zero_moves() {
    let composition = compose(Some(volume()));
    seed_legacy_home(&composition.default_home);
    let user_skill_md =
        std::fs::read_to_string(composition.default_home.join("skills/user-skill/SKILL.md"))
            .expect("user content");

    let snapshot = composition.bootstrap.inspect();
    assert!(
        matches!(snapshot, BootstrapSnapshot::LegacyDetected { .. }),
        "expected LegacyDetected, got {snapshot:?}"
    );
    let candidate = composition
        .binding
        .prepare_home(&composition.default_home)
        .expect("prepare in place");
    assert_eq!(candidate.mode, CandidateMode::LegacyInPlace);
    let snapshot = composition
        .binding
        .confirm_home(&candidate.token)
        .expect("confirm");
    assert!(is_bound(&snapshot), "expected Bound, got {snapshot:?}");

    // Zero moves: the same directory, the same user rows and bytes.
    assert_eq!(
        std::fs::read_to_string(composition.default_home.join("skills/user-skill/SKILL.md"))
            .expect("user content"),
        user_skill_md
    );
    assert_bound_home(&composition, &composition.default_home);
    // The pre-migration backup exists for auditability.
    let backups: Vec<String> = std::fs::read_dir(&composition.default_home)
        .expect("Home")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("pre-migration-v4.bak"))
        .collect();
    assert_eq!(backups.len(), 1, "the v4 Catalog must be backed up");
    // The user rows survived the migration.
    let connection =
        Connection::open(composition.default_home.join(CATALOG_FILE_NAME)).expect("Catalog");
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 1);
}

#[test]
fn legacy_in_place_continue_after_crash_converges() {
    let composition = compose(Some(volume()));
    seed_legacy_home(&composition.default_home);
    let home_id = "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
    let ledger_file = crash_operation(
        &composition,
        "hb-crash",
        "home_candidate",
        home_id,
        &composition.default_home,
        None,
        "preparing",
    );
    write_ledger(&composition, &ledger_file);
    let snapshot = composition
        .binding
        .continue_candidate("hb-crash")
        .expect("continue");
    assert!(is_bound(&snapshot), "expected Bound, got {snapshot:?}");
    let current = AppStateStoreFileSystem::new(composition.state_dir.clone())
        .load()
        .expect("app state")
        .binding
        .current
        .expect("binding");
    assert_eq!(current.home_id.0, home_id);
    // User rows are intact after the converged migration.
    let connection =
        Connection::open(composition.default_home.join(CATALOG_FILE_NAME)).expect("Catalog");
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 1);
}

// -- Legacy copy ------------------------------------------------------------

#[test]
fn legacy_choose_copies_then_binds_and_keeps_the_source_inert() {
    let composition = compose(Some(volume()));
    seed_legacy_home(&composition.default_home);
    let destination = composition.home_root().join("custom-home");

    let candidate = composition
        .binding
        .prepare_home(&destination)
        .expect("prepare copy");
    assert_eq!(candidate.mode, CandidateMode::LegacyCopy);
    assert_eq!(
        candidate.legacy_source.as_deref(),
        Some(composition.default_home.as_path())
    );
    let snapshot = composition
        .binding
        .confirm_home(&candidate.token)
        .expect("confirm");
    assert!(is_bound(&snapshot), "expected Bound, got {snapshot:?}");

    // The destination is a complete, verified Home.
    assert_bound_home(&composition, &destination);
    let connection = Connection::open(destination.join(CATALOG_FILE_NAME)).expect("Catalog");
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))
        .expect("count");
    assert_eq!(count, 1, "user rows must be copied");
    assert_eq!(
        std::fs::read_to_string(destination.join("skills/user-skill/SKILL.md")).expect("copy"),
        "# User skill\n\nReal legacy content.\n"
    );
    // The source is untouched and inert: same bytes, never deleted.
    assert_eq!(
        std::fs::read_to_string(composition.default_home.join("skills/user-skill/SKILL.md"))
            .expect("source"),
        "# User skill\n\nReal legacy content.\n"
    );
    assert!(composition.default_home.join(CATALOG_FILE_NAME).exists());
}

#[test]
fn legacy_copy_continue_after_crash_replaces_partial_copy() {
    let composition = compose(Some(volume()));
    seed_legacy_home(&composition.default_home);
    let destination = composition.home_root().join("custom-home");
    let home_id = "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
    let ledger_file = crash_operation(
        &composition,
        "hb-crash",
        "legacy_transition",
        home_id,
        &composition.default_home,
        Some(&destination),
        "preparing",
    );
    write_ledger(&composition, &ledger_file);
    // A partial copy from the interrupted run.
    std::fs::create_dir_all(destination.join("skills/user-skill")).expect("partial copy");
    std::fs::write(destination.join("skills/user-skill/SKILL.md"), "partial").expect("partial");

    let snapshot = composition
        .binding
        .continue_candidate("hb-crash")
        .expect("continue");
    assert!(is_bound(&snapshot), "expected Bound, got {snapshot:?}");
    let current = AppStateStoreFileSystem::new(composition.state_dir.clone())
        .load()
        .expect("app state")
        .binding
        .current
        .expect("binding");
    assert_eq!(current.home_id.0, home_id);
    assert_eq!(
        std::fs::read_to_string(destination.join("skills/user-skill/SKILL.md")).expect("copy"),
        "# User skill\n\nReal legacy content.\n",
        "the partial copy must be replaced by the verified one"
    );
}

#[test]
fn legacy_copy_cancel_deletes_destination_but_never_the_source() {
    let composition = compose(Some(volume()));
    seed_legacy_home(&composition.default_home);
    let destination = composition.home_root().join("custom-home");
    let home_id = "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
    let ledger_file = crash_operation(
        &composition,
        "hb-crash",
        "legacy_transition",
        home_id,
        &composition.default_home,
        Some(&destination),
        "copied",
    );
    write_ledger(&composition, &ledger_file);
    std::fs::create_dir_all(&destination).expect("copy dir");
    std::fs::write(destination.join("partial.txt"), "copy residue").expect("residue");

    let snapshot = composition
        .binding
        .cancel_candidate("hb-crash")
        .expect("cancel");
    assert!(
        matches!(snapshot, BootstrapSnapshot::LegacyDetected { .. }),
        "expected LegacyDetected, got {snapshot:?}"
    );
    assert!(
        !destination.exists(),
        "the copy destination must be removed"
    );
    assert!(
        composition.default_home.join(CATALOG_FILE_NAME).exists(),
        "the Legacy source must never be touched"
    );
    assert!(ledger(&composition).active.is_none());
}

#[test]
fn prepared_recovery_home_binds_in_place() {
    // A fixture-recovery prepared Home is a v5 Catalog without identity:
    // inspect classifies it as a Legacy-shaped unbound Home and the in-place
    // transition records identity without a migration.
    let composition = compose(Some(volume()));
    std::fs::create_dir_all(&composition.default_home).expect("prepared Home");
    for directory in ["skills", "remotes", "operations", "cache", "staging"] {
        std::fs::create_dir_all(composition.default_home.join(directory)).expect("layout");
    }
    let _ = SqliteCatalogStore::open(&composition.default_home.join(CATALOG_FILE_NAME))
        .expect("prepared Catalog");
    let connection =
        Connection::open(composition.default_home.join(CATALOG_FILE_NAME)).expect("Catalog");
    connection
        .execute(
            "INSERT INTO skills (
                id, directory_name, identity_key, display_name, description,
                source_kind, library_entry_path, final_entity_path, health,
                created_at, updated_at
             ) VALUES (
                'user-skill', 'user-skill', 'user-skill', 'User skill',
                'Recovered content.', 'file_install', '/old/skills/user-skill',
                '/old/skills/user-skill', 'healthy', '2026-07-01T00:00:00Z',
                '2026-07-01T00:00:00Z'
             )",
            [],
        )
        .expect("seed recovered row");
    drop(connection);

    match composition.bootstrap.inspect() {
        BootstrapSnapshot::LegacyDetected { .. } => {}
        other => panic!("expected LegacyDetected for the prepared Home, got {other:?}"),
    }
    let candidate = composition
        .binding
        .prepare_home(&composition.default_home)
        .expect("prepare");
    assert_eq!(candidate.mode, CandidateMode::LegacyInPlace);
    let snapshot = composition
        .binding
        .confirm_home(&candidate.token)
        .expect("confirm");
    assert!(is_bound(&snapshot), "expected Bound, got {snapshot:?}");
    assert_bound_home(&composition, &composition.default_home);
}

#[test]
fn inspect_surfaces_pending_candidate_route() {
    let composition = compose(Some(volume()));
    let home_id = "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";
    let ledger_file = crash_operation(
        &composition,
        "hb-crash",
        "home_candidate",
        home_id,
        &composition.default_home,
        None,
        "created",
    );
    write_ledger(&composition, &ledger_file);
    match composition.bootstrap.inspect() {
        BootstrapSnapshot::HomeCandidatePending { path, operation_id } => {
            assert_eq!(path, composition.default_home);
            assert_eq!(operation_id, "hb-crash");
        }
        other => panic!("expected HomeCandidatePending, got {other:?}"),
    }

    let ledger_file = crash_operation(
        &composition,
        "hb-copy-crash",
        "legacy_transition",
        home_id,
        &composition.default_home,
        Some(&composition.home_root().join("custom-home")),
        "copied",
    );
    write_ledger(&composition, &ledger_file);
    match composition.bootstrap.inspect() {
        BootstrapSnapshot::HomeCandidatePending { path, operation_id } => {
            assert_eq!(path, composition.home_root().join("custom-home"));
            assert_eq!(operation_id, "hb-copy-crash");
        }
        other => panic!("expected HomeCandidatePending for the copy transition, got {other:?}"),
    }
}
