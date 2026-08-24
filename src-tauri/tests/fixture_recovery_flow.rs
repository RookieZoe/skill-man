//! Fixture Recovery flow integration tests (spec §5.2, §10.1): the real
//! adapters — AppStateStore, Catalog probe, FileSystem, prepared Catalog
//! factory and the bootstrap authority — driven end to end. Legacy-unbound
//! and Bound-Restore identity behaviors are both proven against isolated
//! temp Homes, plus crash convergence at every durable cursor.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqlitePreparedCatalogFactory;
use skill_man_lib::core::bootstrap::{
    BootstrapConfig, BootstrapService, BootstrapSnapshot, CatalogAccess,
};
use skill_man_lib::core::fixture_recovery::{
    FixtureClassification, FixtureRecoveryError, FixtureRecoverySelection, FixtureRecoveryService,
    FixtureShapeMode, SystemFixtureClassifier, classify_fixture,
};
use skill_man_lib::core::home::VolumeIdentity;
use skill_man_lib::seams::app_state_store::{AppStateStore, RecoveryLedgerFile};
use skill_man_lib::seams::catalog_probe::{CatalogHomeIdentity, CatalogProbe};
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::seams::prepared_catalog::PreparedCatalogFactory;
use skill_man_lib::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

use common::{CATALOG_FILE_NAME, FixtureHome};

const TEST_FSID: &str = "test-fsid";
const TEST_UUID: &str = "test-uuid";

struct FixedVolumeIdentity(Option<VolumeIdentity>);

impl VolumeIdentitySource for FixedVolumeIdentity {
    fn volume_identity(
        &self,
        _path: &std::path::Path,
    ) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
        Ok(self.0.clone())
    }
}

fn recovery_service(
    state_dir: PathBuf,
    default_home_path: PathBuf,
    volume: Option<VolumeIdentity>,
) -> FixtureRecoveryService {
    let probe = Arc::new(SqliteCatalogProbe::new());
    let filesystem = Arc::new(MacOsFileSystem::new(
        default_home_path
            .parent()
            .and_then(|parent| parent.parent())
            .unwrap_or(&default_home_path)
            .to_path_buf(),
    ));
    let classifier = Arc::new(SystemFixtureClassifier::new(
        probe.clone(),
        filesystem.clone(),
        CATALOG_FILE_NAME.into(),
    ));
    let app_state = Arc::new(AppStateStoreFileSystem::new(state_dir.clone()));
    let bootstrap = Arc::new(BootstrapService::new(
        app_state.clone(),
        Arc::new(FixedVolumeIdentity(volume)),
        probe.clone(),
        filesystem.clone(),
        classifier,
        BootstrapConfig {
            state_dir: state_dir.clone(),
            default_home_path: default_home_path.clone(),
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    ));
    FixtureRecoveryService::new(
        app_state,
        probe,
        filesystem,
        Arc::new(SqlitePreparedCatalogFactory),
        bootstrap,
        BootstrapConfig {
            state_dir,
            default_home_path,
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    )
}

fn sibling(live: &Path, kind: &str, operation_id: &str) -> PathBuf {
    let name = live.file_name().unwrap().to_string_lossy().into_owned();
    live.parent()
        .unwrap()
        .join(format!("{name}.{kind}-{operation_id}"))
}

fn external_probe_json(state_dir: &Path) -> String {
    let mut entries: Vec<(String, u64)> = std::fs::read_dir(state_dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.metadata().unwrap().len(),
            )
        })
        .filter(|(name, _)| name != "recovery-ledger.json" && name != "recovery-ledger.json.tmp")
        .collect();
    entries.sort();
    serde_json::to_string(&entries).unwrap()
}

fn manifest_of(filesystem: &MacOsFileSystem, root: &Path) -> String {
    filesystem
        .tree_hash_excluding(
            root,
            &[
                format!("{CATALOG_FILE_NAME}-wal"),
                format!("{CATALOG_FILE_NAME}-shm"),
            ],
        )
        .unwrap()
}

fn set_ledger(state_dir: &Path, f: impl FnOnce(&mut RecoveryLedgerFile)) {
    let store = AppStateStoreFileSystem::new(state_dir.to_path_buf());
    let mut files = store.load().unwrap();
    f(&mut files.recovery_ledger);
    store.write_recovery_ledger(&files.recovery_ledger).unwrap();
}

fn build_prepared_layout(prepared: &Path) {
    for directory in ["skills", "remotes", "operations", "cache", "staging"] {
        std::fs::create_dir_all(prepared.join(directory)).unwrap();
    }
}

fn prepared_catalog(prepared: &Path, identity: Option<&CatalogHomeIdentity>) {
    SqlitePreparedCatalogFactory
        .create_prepared(&prepared.join(CATALOG_FILE_NAME), identity)
        .unwrap();
}

fn classify_home(
    filesystem: &MacOsFileSystem,
    home_root: &Path,
    mode: FixtureShapeMode,
) -> FixtureClassification {
    let probe = SqliteCatalogProbe::new();
    let collector =
        skill_man_lib::core::fixture_recovery::FixtureEvidenceCollector::new(&probe, filesystem);
    let (db, tree) = collector.collect(home_root, CATALOG_FILE_NAME).unwrap();
    classify_fixture(&db, &tree, home_root, mode)
}

// ---------------------------------------------------------------------------
// Legacy-unbound flow
// ---------------------------------------------------------------------------

#[test]
fn legacy_flow_recovers_to_a_clean_unbound_home() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);

    // Bootstrap locks the contaminated Legacy Home.
    let preview = recovery.inspect().expect("preview");
    assert_eq!(
        preview.mode,
        skill_man_lib::core::fixture_recovery::RecoveryMode::LegacyUnbound
    );
    assert_eq!(preview.classification, FixtureClassification::Pure);
    assert!(preview.can_preview);
    assert!(preview.active_operation.is_none());

    // Plan then apply: the whole Home is isolated and rebuilt.
    let plan = recovery.plan(&FixtureRecoverySelection {}).expect("plan");
    let result = recovery.apply(&plan.plan_token).expect("apply");
    assert!(result.awaiting_commit);
    assert!(!result.rolled_back);
    assert!(
        home.library_root.is_dir(),
        "the recovered live Home is in place awaiting commit"
    );
    let snapshot_path = sibling(&home.library_root, "snapshot", &plan.plan_token);
    assert!(snapshot_path.is_dir(), "Safety Snapshot exists");
    assert!(
        !home.library_root.join("fixture-entities").exists(),
        "the live Home is already clean while awaiting commit"
    );

    // The bootstrap keeps the recovery lock while the op awaits commit.
    let preview = recovery.inspect().expect("preview while active");
    let active = preview.active_operation.expect("active operation");
    assert_eq!(active.cursor.as_deref(), Some("verified"));

    // Commit: the Home is clean and unbound, but its v5 Catalog has no
    // identity and therefore remains Default Home Recovery Blocked.
    let snapshot = recovery.confirm_result(&plan.plan_token).expect("confirm");
    match snapshot {
        BootstrapSnapshot::DefaultHomeRecoveryBlocked { path, .. } => {
            assert_eq!(path, home.library_root);
        }
        other => panic!("expected DefaultHomeRecoveryBlocked, got {other:?}"),
    }
    let filesystem = MacOsFileSystem::new(home.dir.path().to_path_buf());
    assert_eq!(
        classify_home(&filesystem, &home.library_root, FixtureShapeMode::Legacy),
        FixtureClassification::Clean,
        "the recovered Home has no fixture footprint"
    );
    assert!(
        snapshot_path.is_dir(),
        "Safety Snapshot is never auto-deleted"
    );
    assert!(
        !home.library_root.join("fixture-entities").exists(),
        "fixture entities are gone from the recovered Home"
    );
    let ledger = AppStateStoreFileSystem::new(home.state_dir.clone())
        .load()
        .unwrap();
    assert!(ledger.recovery_ledger.active.is_none());
    assert_eq!(
        ledger
            .recovery_ledger
            .completed
            .last()
            .unwrap()
            .cursor
            .as_deref(),
        Some("committed")
    );

    // Snapshot management stays available after the commit.
    let snapshots = recovery.list_snapshots().expect("list snapshots");
    assert_eq!(snapshots.len(), 1);
    let delete = recovery
        .plan_delete_snapshot(&snapshots[0].snapshot_id)
        .expect("plan delete");
    assert!(delete.file_count > 0);
    recovery
        .apply_delete_snapshot(&delete.snapshot_id)
        .expect("apply delete");
    assert!(!snapshot_path.exists(), "user-confirmed delete removes it");
    assert!(recovery.list_snapshots().unwrap().is_empty());
}

#[test]
fn mixed_default_fixture_evidence_is_blocked_not_a_fixture_recovery_bypass() {
    let home = FixtureHome::new();
    home.with_sql("modify fixture row", |connection| {
        connection
            .execute(
                "UPDATE skills SET updated_at = '2026-08-13T10:07:02.781Z' WHERE id = 'media-xray'",
                [],
            )
            .expect("modify row");
    });
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    assert!(matches!(
        recovery.inspect(),
        Err(FixtureRecoveryError::NotLocked)
    ));
    match recovery.plan(&FixtureRecoverySelection {}) {
        Err(FixtureRecoveryError::NotLocked) => {}
        other => panic!("expected NotLocked, got {other:?}"),
    }
    let ledger = AppStateStoreFileSystem::new(home.state_dir.clone())
        .load()
        .unwrap();
    assert!(ledger.recovery_ledger.active.is_none(), "zero artifacts");
}

#[test]
fn double_plan_is_refused_while_an_operation_is_active() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    let plan = recovery.plan(&FixtureRecoverySelection {}).expect("plan");
    match recovery.plan(&FixtureRecoverySelection {}) {
        Err(FixtureRecoveryError::OperationAlreadyActive { .. }) => {}
        other => panic!("expected OperationAlreadyActive, got {other:?}"),
    }
    // Unknown tokens are refused; the real one resumes.
    match recovery.apply("fr-bogus") {
        Err(FixtureRecoveryError::NoActiveOperation) => {}
        other => panic!("expected NoActiveOperation, got {other:?}"),
    }
    recovery.apply(&plan.plan_token).expect("apply");
}

#[test]
fn active_writer_blocks_apply_without_mutating_anything() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);

    // Lock-holder mode: this same test binary acts as the "old Skill Man"
    // process holding the WAL-index lock. POSIX record locks never conflict
    // within one process, so a real child process is the only honest probe.
    if std::env::var("LOCK_HOLDER_MODE").as_deref() == Ok("1") {
        let database = std::env::var("LOCK_HOLDER_DB").expect("LOCK_HOLDER_DB");
        let ready = std::env::var("LOCK_HOLDER_READY").expect("LOCK_HOLDER_READY");
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
    let ready_file = home.dir.path().join("lock-holder-ready");
    let exe = std::env::current_exe().expect("test binary");
    let mut child = std::process::Command::new(exe)
        .arg("--exact")
        .arg("active_writer_blocks_apply_without_mutating_anything")
        .env("LOCK_HOLDER_MODE", "1")
        .env("LOCK_HOLDER_DB", home.catalog_path())
        .env("LOCK_HOLDER_READY", &ready_file)
        .spawn()
        .expect("spawn lock holder");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready_file.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the lock holder never reported ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let plan = recovery.plan(&FixtureRecoverySelection {}).expect("plan");
    match recovery.apply(&plan.plan_token) {
        Err(FixtureRecoveryError::WriterActive(_)) => {}
        other => panic!("expected WriterActive, got {other:?}"),
    }
    assert!(
        home.library_root.exists(),
        "no mutation happened while the writer held the lock"
    );
    child.kill().expect("stop lock holder");
    let _ = child.wait();

    // Once released, the same operation proceeds.
    let result = recovery
        .apply(&plan.plan_token)
        .expect("apply after release");
    assert!(result.awaiting_commit);
}

// ---------------------------------------------------------------------------
// Bound-Restore flow
// ---------------------------------------------------------------------------

/// Contaminate a verified Bound Home with the exact v5 fixture shape: three
/// fixture Agents, three fixture Skills pointing into `fixture-entities/`,
/// the two entity trees, zero relations.
fn contaminate_bound_home(home: &common::BoundTestHome) {
    for (id, name, kind, skills_path, compatibility) in [
        (
            "claude-code",
            "Claude Code",
            "claude_preset",
            "~/.claude/skills",
            "verified",
        ),
        (
            "codex",
            "Codex",
            "codex_preset",
            "~/.codex/skills",
            "verified",
        ),
        (
            "workbench",
            "Workbench",
            "custom",
            "~/Library/Application Support/workbench/skills",
            "unknown",
        ),
    ] {
        home.with_sql("seed fixture Agent", |connection| {
            connection
                .execute(
                    "INSERT INTO agents (
                        id, name, kind, skills_path, path_identity_key, detected,
                        compatibility, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?7)",
                    rusqlite::params![
                        id,
                        name,
                        kind,
                        skills_path,
                        skills_path.to_lowercase(),
                        compatibility,
                        "1970-01-01T00:00:00Z"
                    ],
                )
                .expect("seed fixture Agent");
        });
    }
    let entities = home.library_root.join("fixture-entities");
    for (name, document) in [
        ("skill-authoring", common::FIXTURE_SKILL_AUTHORING_SKILL_MD),
        ("media-xray", common::FIXTURE_MEDIA_XRAY_SKILL_MD),
    ] {
        std::fs::create_dir_all(entities.join(name)).unwrap();
        std::fs::write(entities.join(name).join("SKILL.md"), document).unwrap();
    }
    let entity = |name: &str| {
        home.library_root
            .join("fixture-entities")
            .join(name)
            .to_string_lossy()
            .into_owned()
    };
    home.with_sql("seed fixture Skills", |connection| {
        connection
            .execute(
                "INSERT INTO skills (
                    id, directory_name, identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path, health,
                    created_at, updated_at
                 ) VALUES
                    ('skill-authoring', 'skill-authoring', 'skill-authoring', 'Skill authoring', 'A precise workflow for building maintainable Agent Skills.', 'link', NULL, ?1, 'healthy', '2026-07-20T10:42:00Z', '2026-07-20T10:42:00Z'),
                    ('media-xray', 'media-xray', 'media-xray', 'Media X-ray', 'Transcribes and inspects local audio and video.', 'remote_install', ?2, ?2, 'healthy', '2026-07-19T14:08:00Z', '2026-07-19T14:08:00Z'),
                    ('legacy-audit', 'legacy-audit', 'legacy-audit', 'Legacy audit', 'Checks an existing skills directory before Adopt.', 'link', NULL, ?3, 'broken', '2026-07-18T03:16:00Z', '2026-07-18T03:16:00Z')",
                rusqlite::params![entity("skill-authoring"), entity("media-xray"), entity("legacy-audit")],
            )
            .expect("seed fixture Skills");
    });
}

#[test]
fn bound_flow_restores_the_same_home_identity() {
    let home = common::BoundTestHome::new();
    contaminate_bound_home(&home);
    let volume = Some(VolumeIdentity {
        fsid: TEST_FSID.into(),
        uuid: TEST_UUID.into(),
    });
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), volume);

    let preview = recovery.inspect().expect("preview");
    match &preview.mode {
        skill_man_lib::core::fixture_recovery::RecoveryMode::BoundRestore { home_id } => {
            assert_eq!(home_id.0, common::HOME_ID);
        }
        other => panic!("expected BoundRestore, got {other:?}"),
    }
    assert_eq!(preview.classification, FixtureClassification::Pure);
    assert!(preview.can_preview);

    let plan = recovery.plan(&FixtureRecoverySelection {}).expect("plan");
    let result = recovery.apply(&plan.plan_token).expect("apply");
    assert!(result.awaiting_commit);
    let snapshot = recovery.confirm_result(&plan.plan_token).expect("confirm");
    match &snapshot {
        BootstrapSnapshot::Bound {
            home_id,
            catalog_access: CatalogAccess::ReadWrite,
            ..
        } => assert_eq!(
            home_id.0,
            common::HOME_ID,
            "the SAME home_id survives Restore"
        ),
        other => panic!("expected Bound ReadWrite, got {other:?}"),
    }
    // The recovered Home carries the same marker and Catalog identity.
    let marker = std::fs::read_to_string(
        home.library_root
            .join(skill_man_lib::core::home::HomeMarker::FILE_NAME),
    )
    .expect("marker");
    assert!(marker.contains(common::HOME_ID));
    let report = SqliteCatalogProbe::new()
        .probe(&home.catalog_path())
        .expect("probe recovered Catalog");
    assert_eq!(
        report
            .home_identity
            .as_ref()
            .map(|identity| identity.home_id.0.as_str()),
        Some(common::HOME_ID)
    );
}

// ---------------------------------------------------------------------------
// Crash convergence at every durable cursor
// ---------------------------------------------------------------------------

/// Crash simulation: build the ledger + filesystem state exactly as if the
/// process died at a recorded cursor, then resume with `apply`.
fn crash_state(home: &FixtureHome, build: impl FnOnce(&Path, &Path, &str, &Path)) -> String {
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    let plan = recovery.plan(&FixtureRecoverySelection {}).expect("plan");
    let op_id = plan.plan_token.clone();
    let live = home.library_root.clone();
    let snapshot = sibling(&live, "snapshot", &op_id);
    let prepared = sibling(&live, "prepared", &op_id);
    build(&live, &snapshot, &op_id, &prepared);
    op_id
}

#[test]
fn crash_after_snapshot_with_validated_prepared_rolls_forward() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    let op_id = crash_state(&home, |live, snapshot, op_id, prepared| {
        std::fs::rename(live, snapshot).expect("snapshot rename");
        build_prepared_layout(prepared);
        prepared_catalog(prepared, None);
        let filesystem = MacOsFileSystem::new(live.parent().unwrap().to_path_buf());
        let manifest = manifest_of(&filesystem, prepared);
        set_ledger(home_state_dir(&home), |ledger| {
            let op = ledger.active.as_mut().expect("active op");
            op.snapshot_path = Some(snapshot.to_path_buf());
            op.prepared_path = Some(prepared.to_path_buf());
            op.manifest_hash = Some(manifest);
            op.external_probe = Some(external_probe_json(home_state_dir(&home)));
            op.cursor = Some("validated".into());
            let _ = op_id;
        });
    });
    let result = recovery.apply(&op_id).expect("resume rolls forward");
    assert!(result.awaiting_commit);
    assert!(home.library_root.is_dir(), "prepared Home promoted");
    assert!(
        !sibling(&home.library_root, "prepared", &op_id).exists(),
        "prepared path consumed by promote"
    );
}

fn home_state_dir(home: &FixtureHome) -> &Path {
    &home.state_dir
}

#[test]
fn crash_after_snapshot_with_tampered_prepared_rolls_back() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    let op_id = crash_state(&home, |live, snapshot, _op_id, prepared| {
        std::fs::rename(live, snapshot).expect("snapshot rename");
        build_prepared_layout(prepared);
        prepared_catalog(prepared, None);
        // A partial/tampered write inside the prepared Home.
        std::fs::write(prepared.join("skills").join("stray.txt"), b"tampered").unwrap();
        set_ledger(home_state_dir(&home), |ledger| {
            let op = ledger.active.as_mut().expect("active op");
            op.snapshot_path = Some(snapshot.to_path_buf());
            op.prepared_path = Some(prepared.to_path_buf());
            op.cursor = Some("snapshotted".into());
        });
    });
    match recovery.apply(&op_id) {
        Err(FixtureRecoveryError::StepFailed { rolled_back, .. }) => {
            assert!(rolled_back, "the failed step must roll back");
        }
        other => panic!("expected rolled-back StepFailed, got {other:?}"),
    }
    assert!(
        home.library_root.is_dir(),
        "the original Home is restored from the snapshot"
    );
    let filesystem = MacOsFileSystem::new(home.dir.path().to_path_buf());
    assert_eq!(
        classify_home(&filesystem, &home.library_root, FixtureShapeMode::Legacy),
        FixtureClassification::Pure,
        "the restored Home is the untouched fixture footprint"
    );
    assert!(
        !sibling(&home.library_root, "prepared", &op_id).exists(),
        "the app-created prepared artifact is removed"
    );
    assert!(
        !sibling(&home.library_root, "snapshot", &op_id).exists(),
        "the snapshot content was restored to the live path"
    );
    let ledger = AppStateStoreFileSystem::new(home.state_dir.clone())
        .load()
        .unwrap();
    assert!(ledger.recovery_ledger.active.is_none());
    assert_eq!(
        ledger
            .recovery_ledger
            .completed
            .last()
            .unwrap()
            .cursor
            .as_deref(),
        Some("rolled_back")
    );
}

#[test]
fn crash_at_validated_with_missing_prepared_rolls_back() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    let op_id = crash_state(&home, |live, snapshot, _op_id, _prepared| {
        std::fs::rename(live, snapshot).expect("snapshot rename");
        set_ledger(home_state_dir(&home), |ledger| {
            let op = ledger.active.as_mut().expect("active op");
            op.snapshot_path = Some(snapshot.to_path_buf());
            op.cursor = Some("validated".into());
        });
    });
    let result = recovery.apply(&op_id).expect("apply");
    assert!(result.rolled_back, "missing prepared converges to rollback");
    assert!(home.library_root.is_dir(), "original Home restored");
    assert!(
        !sibling(&home.library_root, "snapshot", &op_id).exists(),
        "the snapshot was restored to the live path by the rollback"
    );
}

#[test]
fn crash_window_after_snapshot_rename_before_ledger_write_rolls_forward() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    let op_id = crash_state(&home, |live, snapshot, _op_id, _prepared| {
        // The rename happened; the ledger still says `confirmed`.
        std::fs::rename(live, snapshot).expect("snapshot rename");
    });
    let result = recovery.apply(&op_id).expect("apply");
    assert!(result.awaiting_commit, "facts decide: roll forward");
    assert!(home.library_root.is_dir());
    let preview = recovery.inspect().expect("preview");
    assert!(preview.active_operation.is_some(), "op awaiting commit");
}

#[test]
fn snapshot_delete_is_refused_while_the_active_operation_references_it() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    let plan = recovery.plan(&FixtureRecoverySelection {}).expect("plan");
    recovery.apply(&plan.plan_token).expect("apply");
    let snapshots = recovery.list_snapshots().expect("list");
    assert_eq!(snapshots.len(), 1);
    match recovery.plan_delete_snapshot(&snapshots[0].snapshot_id) {
        Err(FixtureRecoveryError::SnapshotInUse) => {}
        other => panic!("expected SnapshotInUse, got {other:?}"),
    }
    recovery.confirm_result(&plan.plan_token).expect("confirm");
    let snapshots = recovery.list_snapshots().expect("list after commit");
    assert_eq!(snapshots.len(), 1, "snapshot survives the commit");
    let delete = recovery
        .plan_delete_snapshot(&snapshots[0].snapshot_id)
        .expect("plan delete after commit");
    recovery
        .apply_delete_snapshot(&delete.snapshot_id)
        .expect("apply delete after commit");
    assert!(recovery.list_snapshots().unwrap().is_empty());
}

#[test]
fn failed_final_verification_rolls_back_instead_of_sticking_at_awaiting_commit() {
    let home = FixtureHome::new();
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), None);
    let plan = recovery.plan(&FixtureRecoverySelection {}).expect("plan");
    recovery.apply(&plan.plan_token).expect("apply");
    assert!(home.library_root.is_dir(), "the clean Home awaits commit");

    // Corruption appears between apply and commit (e.g. a torn disk write):
    // the final verification must fail, roll back to the Safety Snapshot and
    // clear the operation — the user is never stuck at AwaitingCommit.
    std::fs::write(home.catalog_path(), b"not a sqlite database at all").expect("corrupt live");
    match recovery.confirm_result(&plan.plan_token) {
        Err(FixtureRecoveryError::StepFailed { rolled_back, .. }) => {
            assert!(rolled_back, "final verification failure must roll back");
        }
        other => panic!("expected rolled-back StepFailed, got {other:?}"),
    }
    let filesystem = MacOsFileSystem::new(home.dir.path().to_path_buf());
    assert_eq!(
        classify_home(&filesystem, &home.library_root, FixtureShapeMode::Legacy),
        FixtureClassification::Pure,
        "the original fixture Home is restored from the snapshot"
    );
    let ledger = AppStateStoreFileSystem::new(home.state_dir.clone())
        .load()
        .unwrap();
    assert!(ledger.recovery_ledger.active.is_none());
    assert_eq!(
        ledger
            .recovery_ledger
            .completed
            .last()
            .unwrap()
            .cursor
            .as_deref(),
        Some("rolled_back")
    );
}

#[test]
fn recovery_is_not_applicable_outside_the_lock() {
    let dir = tempfile::tempdir().expect("temp dir");
    let state_dir = dir
        .path()
        .join("Library/Application Support/skill-man-state");
    let default_home = dir.path().join("Library/Application Support/skill-man");
    std::fs::create_dir_all(&state_dir).unwrap();
    let recovery = recovery_service(state_dir, default_home, None);
    match recovery.inspect() {
        Err(FixtureRecoveryError::NotLocked) => {}
        other => panic!("expected NotLocked, got {other:?}"),
    }
}

#[test]
fn bound_home_without_fixture_is_not_recoverable() {
    let home = common::BoundTestHome::new();
    let volume = Some(VolumeIdentity {
        fsid: TEST_FSID.into(),
        uuid: TEST_UUID.into(),
    });
    let recovery = recovery_service(home.state_dir.clone(), home.library_root.clone(), volume);
    match recovery.inspect() {
        Err(FixtureRecoveryError::NotLocked) => {}
        other => panic!("expected NotLocked, got {other:?}"),
    }
}
