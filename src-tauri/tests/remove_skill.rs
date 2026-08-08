//! T11 Remove & recovery locking: preview-confirm Remove that disables every
//! Activation first, keeps Link entities but deletes Install entities,
//! cascades catalog cleanup while archiving the operation audit, recovers
//! interrupted Removes in both directions, and locks writes when recovery
//! itself is required.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_man_lib::adapters::agent_adapters::BuiltInAgentAdapters;
use skill_man_lib::adapters::fixture_catalog::FixtureCatalogStore;
use skill_man_lib::adapters::local_file_source::LocalFileSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::activation::{ActivationService, SetActivation};
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::domain::{AgentId, CatalogFilter, SkillId};
use skill_man_lib::core::import::ImportService;
use skill_man_lib::core::maintenance::{MaintenanceError, MaintenanceService};
use skill_man_lib::seams::activation_store::ActivationStore;
use skill_man_lib::seams::filesystem::{
    FileSystem, RemoveActivationStep, RemoveInitialEntry, RemoveJournal, RemoveJournalPhase,
    RemoveRecoveryBaseline, RemoveSourceKind,
};
use skill_man_lib::seams::maintenance_store::MaintenanceStore;
use skill_man_lib::seams::recovery::RecoveryGate;

struct TestHarness {
    home: tempfile::TempDir,
    library_root: PathBuf,
    runtime: Arc<RuntimeCatalogStore>,
    catalog: CatalogService,
    import: ImportService,
    activation: ActivationService,
    maintenance: MaintenanceService,
}

fn harness() -> TestHarness {
    harness_with_gate(Arc::new(RecoveryGate::ready()))
}

fn harness_with_gate(recovery_gate: Arc<RecoveryGate>) -> TestHarness {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let fixture =
        Arc::new(FixtureCatalogStore::runtime(&library_root).expect("materialize fixture"));
    let sqlite = Arc::new(
        SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3")).expect("open SQLite"),
    );
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("seed catalog");
    let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture,
        sqlite,
        filesystem.clone(),
    ));
    let import = ImportService::new(
        runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    )
    .with_recovery_gate(recovery_gate.clone());
    let catalog = CatalogService::new(runtime.clone());
    let activation =
        ActivationService::new(runtime.clone(), filesystem.clone(), library_root.clone())
            .with_agent_adapters(Arc::new(BuiltInAgentAdapters))
            .with_recovery_gate(recovery_gate.clone());
    let maintenance = MaintenanceService::new(runtime.clone(), filesystem)
        .with_library_root(library_root.clone())
        .with_recovery_gate(recovery_gate);
    TestHarness {
        home,
        library_root,
        runtime,
        catalog,
        import,
        activation,
        maintenance,
    }
}

fn write_skill(root: &Path, name: &str, frontmatter_name: &str, description: &str) {
    std::fs::create_dir_all(root).expect("create Skill directory");
    std::fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {frontmatter_name}\ndescription: {description}\n---\n\n# {name}\n"),
    )
    .expect("write SKILL.md");
}

fn import_link_and_enable(harness: &TestHarness, source: &Path, agent: &str) -> SkillId {
    let preview = harness.import.plan_link(source).expect("plan Link Import");
    let imported = harness
        .import
        .apply_link(&preview.plan_token)
        .expect("apply Link Import");
    let activation = harness
        .activation
        .plan(SetActivation {
            skill_id: imported.skill_id.clone(),
            agent_id: AgentId(agent.into()),
            enabled: true,
        })
        .expect("plan Enable");
    harness
        .activation
        .apply(&activation.plan_token)
        .expect("apply Enable");
    imported.skill_id
}

#[test]
fn remove_link_disables_activations_and_keeps_the_external_entity() {
    let harness = harness();
    let source = harness.home.path().join("Projects/removable-skill");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    write_skill(&source, "removable-skill", "removable-skill", "Removable.");
    let skill_id = import_link_and_enable(&harness, &source, "claude-code");

    let preview = harness
        .maintenance
        .plan_remove(&skill_id)
        .expect("plan Remove");
    assert_eq!(preview.directory_name, "removable-skill");
    assert_eq!(preview.activation_count, 1);

    let result = harness
        .maintenance
        .apply_remove(&preview.plan_token)
        .expect("apply Remove");
    assert_eq!(result.directory_name, "removable-skill");

    // The Link entity stays at its external source; the Activation is gone.
    assert!(source.join("SKILL.md").is_file());
    assert!(!agent_root.join("removable-skill").exists());

    // The catalog row and its activations cascade away.
    let skills = harness
        .catalog
        .list(CatalogFilter::All)
        .expect("list Library");
    assert!(!skills.items.iter().any(|skill| skill.id == skill_id));
    assert!(
        ActivationStore::desired_activations(harness.runtime.as_ref())
            .expect("desired Activations")
            .iter()
            .all(|activation| activation.skill_id != skill_id)
    );

    // The operation audit survives as an archived journal.
    let history = std::fs::read_dir(harness.library_root.join("operation-history"))
        .expect("operation history")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(".remove.json")
        })
        .count();
    assert_eq!(history, 1, "the Remove journal must be archived");
    assert!(
        !harness
            .library_root
            .join(format!("operations/remove-{}-{}", 0, skill_id.0))
            .exists()
    );
}

#[test]
fn remove_install_deletes_the_library_entity() {
    let harness = harness();
    let source = harness.home.path().join("Downloads/installed-skill");
    write_skill(&source, "installed-skill", "installed-skill", "Installed.");
    let preview = harness.import.plan_file(&source).expect("plan file Import");
    let imported = harness
        .import
        .apply_file(&preview.plan_token)
        .expect("apply file Import");
    let entity = harness.library_root.join("skills/installed-skill");
    assert!(entity.join("SKILL.md").is_file());

    let remove = harness
        .maintenance
        .plan_remove(&imported.skill_id)
        .expect("plan Remove");
    harness
        .maintenance
        .apply_remove(&remove.plan_token)
        .expect("apply Remove");

    assert!(!entity.exists(), "the Install entity must be deleted");
    assert!(source.join("SKILL.md").is_file(), "the Import source stays");
    let skills = harness
        .catalog
        .list(CatalogFilter::All)
        .expect("list Library");
    assert!(
        !skills
            .items
            .iter()
            .any(|skill| skill.id == imported.skill_id)
    );
}

#[test]
fn remove_broken_link_clears_the_catalog_row() {
    let harness = harness();
    let source = harness.home.path().join("Projects/gone-skill");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    write_skill(&source, "gone-skill", "gone-skill", "Gone.");
    let skill_id = import_link_and_enable(&harness, &source, "claude-code");
    std::fs::remove_dir_all(&source).expect("remove Link source");

    let remove = harness
        .maintenance
        .plan_remove(&skill_id)
        .expect("plan Remove");
    harness
        .maintenance
        .apply_remove(&remove.plan_token)
        .expect("apply Remove of Broken Link");

    let skills = harness
        .catalog
        .list(CatalogFilter::All)
        .expect("list Library");
    assert!(!skills.items.iter().any(|skill| skill.id == skill_id));
}

#[test]
fn remove_stops_when_an_activation_entry_is_occupied_after_preview() {
    let harness = harness();
    let source = harness.home.path().join("Projects/occupied-skill");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    write_skill(&source, "occupied-skill", "occupied-skill", "Occupied.");
    let skill_id = import_link_and_enable(&harness, &source, "claude-code");

    let remove = harness
        .maintenance
        .plan_remove(&skill_id)
        .expect("plan Remove");
    std::fs::remove_file(agent_root.join("occupied-skill")).expect("remove Activation");
    std::fs::create_dir(agent_root.join("occupied-skill")).expect("occupy entry");

    let error = harness
        .maintenance
        .apply_remove(&remove.plan_token)
        .expect_err("occupied entry blocks Remove");
    assert!(
        matches!(error, MaintenanceError::PlanStale),
        "unexpected error: {error}"
    );

    // The catalog row survives untouched.
    let skills = harness
        .catalog
        .list(CatalogFilter::All)
        .expect("list Library");
    assert!(skills.items.iter().any(|skill| skill.id == skill_id));
    assert!(source.join("SKILL.md").is_file());
}

#[test]
fn interrupted_remove_rolls_back_at_startup_when_the_catalog_survives() {
    let harness = harness();
    let source = harness.home.path().join("Downloads/rollback-install");
    write_skill(&source, "rollback-install", "rollback-install", "Rollback.");
    let preview = harness.import.plan_file(&source).expect("plan file Import");
    let imported = harness
        .import
        .apply_file(&preview.plan_token)
        .expect("apply file Import");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    let activation = harness
        .activation
        .plan(SetActivation {
            skill_id: imported.skill_id.clone(),
            agent_id: AgentId("claude-code".into()),
            enabled: true,
        })
        .expect("plan Enable");
    harness
        .activation
        .apply(&activation.plan_token)
        .expect("apply Enable");
    let entity = harness.library_root.join("skills/rollback-install");
    let entry_path = agent_root.join("rollback-install");

    // Simulate a crash after the Activation was removed and the entity
    // backed up, before the catalog commit.
    let entity_fingerprint = std::fs::symlink_metadata(&entity).expect("entity metadata");
    let backup_dir = harness
        .library_root
        .join("operations/remove-crash-test/backup");
    std::fs::create_dir_all(&backup_dir).expect("create backup directory");
    let backup_path = backup_dir.join("rollback-install");
    std::fs::rename(&entity, &backup_path).expect("back up entity");
    std::fs::remove_file(&entry_path).expect("remove Activation");
    let journal = RemoveJournal {
        version: 1,
        operation_id: "remove-crash-test".into(),
        phase: RemoveJournalPhase::Applying,
        skill_id: imported.skill_id.0.clone(),
        source_kind: RemoveSourceKind::Install,
        final_entity_path: entity.clone(),
        backup_path: Some(backup_path.clone()),
        backup_fingerprint: Some(skill_man_lib::seams::filesystem::DirectoryFingerprint {
            canonical_path: backup_path.canonicalize().expect("canonical backup"),
            device: entity_fingerprint.dev(),
            inode: entity_fingerprint.ino(),
        }),
        activations: vec![RemoveActivationStep {
            agent_id: "claude-code".into(),
            entry_path: entry_path.clone(),
            target_path: entity.clone(),
            initial_entry: RemoveInitialEntry::Symlink,
        }],
    };
    let filesystem = MacOsFileSystem::new(harness.home.path().to_path_buf());
    filesystem
        .write_remove_journal(&harness.library_root, &journal)
        .expect("write interrupted journal");

    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("startup recovery completes");

    // Rolled back: the entity is restored and the Activation recreated.
    assert!(entity.join("SKILL.md").is_file(), "entity must be restored");
    assert_eq!(
        std::fs::read_link(&entry_path).expect("recreated Activation"),
        entity,
        "the Activation must point at the restored entity"
    );
    assert!(
        harness
            .library_root
            .join("operation-history/remove-crash-test.remove.json")
            .is_file(),
        "journal must be archived after recovery"
    );
    assert!(
        !harness
            .library_root
            .join("operations/remove-crash-test")
            .exists(),
        "operation directory must be discarded after recovery"
    );
}

#[test]
fn interrupted_remove_rolls_forward_when_the_catalog_row_is_gone() {
    let harness = harness();
    let source = harness.home.path().join("Downloads/forward-install");
    write_skill(&source, "forward-install", "forward-install", "Forward.");
    let preview = harness.import.plan_file(&source).expect("plan file Import");
    let imported = harness
        .import
        .apply_file(&preview.plan_token)
        .expect("apply file Import");
    let entity = harness.library_root.join("skills/forward-install");

    // The catalog committed the delete, but the process died before the
    // backup was discarded.
    let backup_dir = harness
        .library_root
        .join("operations/remove-forward-test/backup");
    std::fs::create_dir_all(&backup_dir).expect("create backup directory");
    let backup_path = backup_dir.join("forward-install");
    std::fs::rename(&entity, &backup_path).expect("back up entity");
    harness
        .runtime
        .delete_skill(&imported.skill_id)
        .expect("commit catalog delete");
    let journal = RemoveJournal {
        version: 1,
        operation_id: "remove-forward-test".into(),
        phase: RemoveJournalPhase::Applying,
        skill_id: imported.skill_id.0.clone(),
        source_kind: RemoveSourceKind::Install,
        final_entity_path: entity.clone(),
        backup_path: Some(backup_path.clone()),
        backup_fingerprint: None,
        activations: Vec::new(),
    };
    let filesystem = MacOsFileSystem::new(harness.home.path().to_path_buf());
    filesystem
        .write_remove_journal(&harness.library_root, &journal)
        .expect("write interrupted journal");

    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("startup recovery completes");

    assert!(!entity.exists(), "entity must stay deleted");
    assert!(!backup_path.exists(), "backup must be discarded");
    assert!(
        harness
            .library_root
            .join("operation-history/remove-forward-test.remove.json")
            .is_file(),
        "journal must be archived after recovery"
    );
}

#[test]
fn unrecoverable_remove_locks_writes_and_keeps_browsing_available() {
    let gate = Arc::new(RecoveryGate::ready());
    let harness = harness_with_gate(gate.clone());
    let source = harness.home.path().join("Projects/locked-skill");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    write_skill(&source, "locked-skill", "locked-skill", "Locked.");
    let skill_id = import_link_and_enable(&harness, &source, "claude-code");

    // Simulate an interrupted Remove whose Activation entry was taken over
    // by external content: rollback can neither proceed nor compensate.
    let entry_path = agent_root.join("locked-skill");
    std::fs::remove_file(&entry_path).expect("remove Activation");
    std::fs::create_dir(&entry_path).expect("occupy entry externally");
    let journal = RemoveJournal {
        version: 1,
        operation_id: "remove-locked-test".into(),
        phase: RemoveJournalPhase::Applying,
        skill_id: skill_id.0.clone(),
        source_kind: RemoveSourceKind::Link,
        final_entity_path: source.canonicalize().expect("canonical source"),
        backup_path: None,
        backup_fingerprint: None,
        activations: vec![RemoveActivationStep {
            agent_id: "claude-code".into(),
            entry_path: entry_path.clone(),
            target_path: source.canonicalize().expect("canonical source"),
            initial_entry: RemoveInitialEntry::Symlink,
        }],
    };
    let filesystem = MacOsFileSystem::new(harness.home.path().to_path_buf());
    filesystem
        .write_remove_journal(&harness.library_root, &journal)
        .expect("write unrecoverable journal");

    // Production starts blocked and only unlocks after recovery succeeds.
    gate.mark_blocked();

    // Startup recovery fails with recovery_required; the gate stays blocked.
    let maintenance = harness.maintenance.clone().begin_startup();
    let error = maintenance
        .run_activation_health_check()
        .expect_err("startup recovery must fail");
    assert!(
        matches!(
            error,
            MaintenanceError::FileSystem(
                skill_man_lib::seams::filesystem::FileSystemError::RecoveryRequired { .. }
            )
        ),
        "unexpected error: {error}"
    );

    // Browsing still works; every write is locked out.
    let skills = harness
        .catalog
        .list(CatalogFilter::All)
        .expect("list Library while locked");
    assert!(skills.items.iter().any(|skill| skill.id == skill_id));
    let write_error = harness
        .maintenance
        .plan_remove(&skill_id)
        .expect_err("writes are locked while recovery is required");
    assert!(
        matches!(write_error, MaintenanceError::RecoveryInProgress),
        "unexpected error: {write_error}"
    );
    let import_error = harness
        .import
        .discover_file(&source)
        .expect_err("Import writes are locked too");
    assert!(
        matches!(
            &import_error,
            skill_man_lib::core::import::ImportError::RecoveryRequired(message)
                if message.contains("startup recovery")
        ),
        "unexpected error: {import_error}"
    );
}

#[test]
fn remove_recovery_baselines_cover_every_managed_skill() {
    let harness = harness();
    let baselines = harness
        .runtime
        .managed_skill_baselines()
        .expect("managed baselines")
        .into_iter()
        .map(|baseline| RemoveRecoveryBaseline {
            skill_id: baseline.skill_id.0,
        })
        .collect::<Vec<_>>();
    assert!(
        baselines
            .iter()
            .any(|baseline| baseline.skill_id == "skill-authoring")
    );
}

#[test]
fn retrying_recovery_after_repair_unlocks_writes() {
    let gate = Arc::new(RecoveryGate::ready());
    let harness = harness_with_gate(gate.clone());
    let source = harness.home.path().join("Projects/retry-skill");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    write_skill(&source, "retry-skill", "retry-skill", "Retry.");
    let skill_id = import_link_and_enable(&harness, &source, "claude-code");
    let entity = source.canonicalize().expect("canonical source");

    // An interrupted Remove whose entry is externally occupied cannot be
    // recovered; retrying must fail and keep writes locked.
    let entry_path = agent_root.join("retry-skill");
    std::fs::remove_file(&entry_path).expect("remove Activation");
    std::fs::create_dir(&entry_path).expect("occupy entry externally");
    let journal = RemoveJournal {
        version: 1,
        operation_id: "remove-retry-test".into(),
        phase: RemoveJournalPhase::Applying,
        skill_id: skill_id.0.clone(),
        source_kind: RemoveSourceKind::Link,
        final_entity_path: entity.clone(),
        backup_path: None,
        backup_fingerprint: None,
        activations: vec![RemoveActivationStep {
            agent_id: "claude-code".into(),
            entry_path: entry_path.clone(),
            target_path: entity.clone(),
            initial_entry: RemoveInitialEntry::Symlink,
        }],
    };
    let filesystem = MacOsFileSystem::new(harness.home.path().to_path_buf());
    filesystem
        .write_remove_journal(&harness.library_root, &journal)
        .expect("write interrupted journal");
    gate.mark_blocked();

    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect_err("first recovery attempt fails on the occupied entry");
    harness
        .maintenance
        .plan_remove(&skill_id)
        .expect_err("writes stay locked after a failed recovery");

    // The user removes the external occupant; retrying recovery rolls the
    // journal back and unlocks writes.
    std::fs::remove_dir(&entry_path).expect("clear the external occupant");
    maintenance
        .run_activation_health_check()
        .expect("retried recovery completes after the repair");

    assert_eq!(
        std::fs::read_link(&entry_path).expect("rolled-back Activation"),
        entity,
        "the Activation must be recreated by the rollback"
    );
    assert!(
        harness
            .library_root
            .join("operation-history/remove-retry-test.remove.json")
            .is_file(),
        "the recovered journal must be archived"
    );
    harness
        .maintenance
        .plan_remove(&skill_id)
        .expect("writes are unlocked after a successful retry");
}
