//! T10 Broken/Modified recovery: Link health detection, the Relocate flow
//! (SKILL.md + directory identity + frontmatter matching + preview confirm),
//! pointer/Activation updates with health recovery, the Broken vs Modified
//! distinction for Install entities, and journal recovery of interrupted
//! relocations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_man_lib::adapters::agent_adapters::BuiltInAgentAdapters;
use skill_man_lib::adapters::local_file_source::LocalFileSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::activation::{ActivationService, SetActivation};
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::domain::{AgentId, Health, SkillId};
use skill_man_lib::core::import::ImportService;
use skill_man_lib::core::maintenance::{MaintenanceError, MaintenanceService};
use skill_man_lib::seams::filesystem::{
    FileSystem, RelocateActivationStep, RelocateInitialEntry, RelocateJournal, RelocateJournalPhase,
};
use skill_man_lib::seams::maintenance_store::MaintenanceStore;

mod common;
use common::BoundTestHome;

struct TestHarness {
    home: BoundTestHome,
    library_root: PathBuf,
    runtime: Arc<RuntimeCatalogStore>,
    catalog: CatalogService,
    import: ImportService,
    activation: ActivationService,
    maintenance: MaintenanceService,
}

fn harness() -> TestHarness {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let library_root = home.library_root.clone();
    let filesystem = home.filesystem.clone();
    let runtime = home.runtime.clone();
    let import = ImportService::new(
        runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(LocalFileSource::new()),
        library_root.clone(),
    );
    let catalog = CatalogService::new(runtime.clone());
    let activation =
        ActivationService::new(runtime.clone(), filesystem.clone(), library_root.clone())
            .with_agent_adapters(Arc::new(BuiltInAgentAdapters));
    let maintenance = MaintenanceService::new(runtime.clone(), filesystem)
        .with_library_root(library_root.clone());
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

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().expect("canonical path")
}

/// Import a Link Skill and enable it on the given Agents. Returns its id.
fn import_and_enable(harness: &TestHarness, source: &Path, agents: &[&str]) -> SkillId {
    let preview = harness.import.plan_link(source).expect("plan Link Import");
    let imported = harness
        .import
        .apply_link(&preview.plan_token)
        .expect("apply Link Import");
    for agent in agents {
        let activation = harness
            .activation
            .plan(SetActivation {
                skill_id: imported.skill_id.clone(),
                agent_id: AgentId((*agent).into()),
                enabled: true,
            })
            .expect("plan Enable");
        harness
            .activation
            .apply(&activation.plan_token)
            .expect("apply Enable");
    }
    imported.skill_id
}

#[test]
fn link_source_removal_marks_broken_and_relocation_restores_pointer_activations_and_health() {
    let harness = harness();
    let source = harness.home.path().join("Projects/authoring-skill");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    write_skill(
        &source,
        "authoring-skill",
        "authoring-skill",
        "Author linked skills.",
    );

    let skill_id = import_and_enable(&harness, &source, &["claude-code"]);
    let old_entity = canonical(&source);

    // The source disappears -> the Link is Broken after a health check.
    std::fs::remove_dir_all(&source).expect("remove Link source");
    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("health check");
    let detail = harness
        .catalog
        .inspect(skill_id.clone())
        .expect("inspect Broken Link");
    assert_eq!(detail.summary.health, Health::Broken);

    // A relocated source with the same directory identity and frontmatter.
    let replacement = harness.home.path().join("Projects/moved/authoring-skill");
    write_skill(
        &replacement,
        "authoring-skill",
        "authoring-skill",
        "Author linked skills.",
    );

    let relocate = harness
        .maintenance
        .relocate(&skill_id, &replacement)
        .expect("relocate preview");
    assert_eq!(relocate.directory_name, "authoring-skill");
    assert_eq!(relocate.activation_count, 1);
    assert_eq!(relocate.final_entity_path, canonical(&replacement));
    let applied = harness
        .maintenance
        .apply_relocate(&relocate.plan_token)
        .expect("apply relocate");
    assert_eq!(applied.activation_count, 1);

    let new_entity = canonical(&replacement);
    let detail = harness
        .catalog
        .inspect(skill_id)
        .expect("inspect relocated Link");
    assert_eq!(detail.summary.health, Health::Healthy);
    assert_eq!(detail.final_entity_path, new_entity.to_string_lossy());
    assert_eq!(
        std::fs::read_link(agent_root.join("authoring-skill")).expect("Activation"),
        new_entity
    );
    assert_ne!(new_entity, old_entity);
}

#[test]
fn relocate_rejects_mismatched_identity_frontmatter_and_unreadable_sources() {
    let harness = harness();
    let source = harness.home.path().join("Projects/strict-skill");
    write_skill(&source, "strict-skill", "strict-skill", "Identity checks.");
    let skill_id = import_and_enable(&harness, &source, &[]);

    // Different directory name.
    let renamed = harness.home.path().join("Projects/renamed-skill");
    write_skill(
        &renamed,
        "renamed-skill",
        "strict-skill",
        "Identity checks.",
    );
    let error = harness
        .maintenance
        .relocate(&skill_id, &renamed)
        .expect_err("directory identity must be preserved");
    assert!(
        error.to_string().contains("must keep the identity"),
        "unexpected error: {error}"
    );

    // Different frontmatter name (frontmatter exists and must match).
    let renamed_frontmatter = harness.home.path().join("Projects/moved/strict-skill");
    write_skill(
        &renamed_frontmatter,
        "strict-skill",
        "other-name",
        "Identity checks.",
    );
    let error = harness
        .maintenance
        .relocate(&skill_id, &renamed_frontmatter)
        .expect_err("frontmatter name must match");
    assert!(
        error.to_string().contains("frontmatter name"),
        "unexpected error: {error}"
    );

    // Unreadable source (no SKILL.md).
    let no_document = harness.home.path().join("Projects/elsewhere/strict-skill");
    std::fs::create_dir_all(&no_document).expect("create source without SKILL.md");
    let error = harness
        .maintenance
        .relocate(&skill_id, &no_document)
        .expect_err("source must contain a readable SKILL.md");
    assert!(
        error.to_string().contains("not a readable Skill"),
        "unexpected error: {error}"
    );
}

#[test]
fn relocate_plan_goes_stale_when_the_new_source_changes_after_preview() {
    let harness = harness();
    let source = harness.home.path().join("Projects/changing-skill");
    write_skill(&source, "changing-skill", "changing-skill", "Changes.");
    let skill_id = import_and_enable(&harness, &source, &[]);
    std::fs::remove_dir_all(&source).expect("remove Link source");
    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("health check");

    let replacement = harness
        .home
        .path()
        .join("Projects/relocated/changing-skill");
    write_skill(&replacement, "changing-skill", "changing-skill", "Changes.");
    let relocate = match harness.maintenance.relocate(&skill_id, &replacement) {
        Ok(preview) => preview,
        Err(error) => panic!("relocate preview failed: {error}"),
    };
    std::fs::remove_file(replacement.join("SKILL.md")).expect("change source after preview");

    let error = harness
        .maintenance
        .apply_relocate(&relocate.plan_token)
        .expect_err("changed source makes the plan stale");
    assert!(
        matches!(error, MaintenanceError::PlanStale),
        "unexpected error: {error}"
    );
    let detail = harness
        .catalog
        .inspect(skill_id)
        .expect("inspect unchanged Link");
    assert_eq!(detail.summary.health, Health::Broken);
}

#[test]
fn install_entity_missing_is_broken_while_content_changes_are_modified() {
    let harness = harness();
    let source = harness.home.path().join("Downloads/file-authoring");
    write_skill(&source, "file-authoring", "file-authoring", "A snapshot.");
    let preview = harness.import.plan_file(&source).expect("plan file Import");
    let imported = harness
        .import
        .apply_file(&preview.plan_token)
        .expect("apply file Import");

    // Content edit -> Modified.
    let entity = harness.library_root.join("skills/file-authoring");
    std::fs::write(entity.join("SKILL.md"), "# Edited\n").expect("edit SKILL.md");
    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("health check");
    let detail = harness
        .catalog
        .inspect(imported.skill_id.clone())
        .expect("inspect Modified Install");
    assert_eq!(detail.summary.health, Health::Modified);

    // Entity removed -> Broken, never Modified.
    std::fs::remove_dir_all(&entity).expect("remove Install entity");
    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("health check");
    let detail = harness
        .catalog
        .inspect(imported.skill_id)
        .expect("inspect Broken Install");
    assert_eq!(detail.summary.health, Health::Broken);
    assert!(detail.skill_markdown.is_empty());
}

#[test]
fn relocation_compensates_an_occupied_activation_entry_after_partial_rewrite() {
    let harness = harness();
    let source = harness.home.path().join("Projects/two-agent-skill");
    let claude_root = harness.home.path().join(".claude/skills");
    let codex_root = harness.home.path().join(".codex/skills");
    std::fs::create_dir_all(&claude_root).expect("create Claude directory");
    std::fs::create_dir_all(&codex_root).expect("create Codex directory");
    write_skill(&source, "two-agent-skill", "two-agent-skill", "Two Agents.");
    let skill_id = import_and_enable(&harness, &source, &["claude-code", "codex"]);
    let old_entity = canonical(&source);
    std::fs::remove_dir_all(&source).expect("remove Link source");
    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("health check");

    let replacement = harness
        .home
        .path()
        .join("Projects/relocated/two-agent-skill");
    write_skill(
        &replacement,
        "two-agent-skill",
        "two-agent-skill",
        "Two Agents.",
    );
    let relocate = harness
        .maintenance
        .relocate(&skill_id, &replacement)
        .expect("relocate preview");
    assert_eq!(relocate.activation_count, 2);

    // Occupy the Codex entry after preview; the Claude entry is rewritten
    // first and must be compensated back.
    std::fs::remove_file(codex_root.join("two-agent-skill")).expect("remove Codex Activation");
    std::fs::create_dir(codex_root.join("two-agent-skill")).expect("occupy Codex entry");

    let error = harness
        .maintenance
        .apply_relocate(&relocate.plan_token)
        .expect_err("occupied entry blocks relocation");
    assert!(
        matches!(error, MaintenanceError::PlanStale),
        "unexpected error: {error}"
    );

    // The Claude entry returned to its original (now dangling) target and the
    // catalog pointer never moved.
    assert_eq!(
        std::fs::read_link(claude_root.join("two-agent-skill")).expect("compensated Claude entry"),
        old_entity
    );
    assert!(codex_root.join("two-agent-skill").is_dir());
    let detail = harness
        .catalog
        .inspect(skill_id)
        .expect("inspect unchanged Link");
    assert_eq!(detail.summary.health, Health::Broken);
    assert_eq!(detail.final_entity_path, old_entity.to_string_lossy());
}

#[test]
fn interrupted_relocation_rolls_back_at_startup_when_the_catalog_never_committed() {
    let harness = harness();
    let source = harness.home.path().join("Projects/rollback-skill");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    write_skill(&source, "rollback-skill", "rollback-skill", "Rollback.");
    let skill_id = import_and_enable(&harness, &source, &["claude-code"]);
    let old_entity = canonical(&source);
    let entry_path = agent_root.join("rollback-skill");
    std::fs::remove_dir_all(&source).expect("remove Link source");

    // Simulate a crash after the first Activation was rewritten: the journal
    // is Applying and the catalog still records the old pointer.
    let replacement = harness.home.path().join("Projects/rollback-skill-new");
    write_skill(
        &replacement,
        "rollback-skill",
        "rollback-skill",
        "Rollback.",
    );
    let new_entity = canonical(&replacement);
    std::fs::remove_file(&entry_path).expect("remove old Activation");
    std::os::unix::fs::symlink(&new_entity, &entry_path).expect("rewrite Activation");
    let journal = RelocateJournal {
        version: 1,
        operation_id: "relocate-rollback-test".into(),
        phase: RelocateJournalPhase::Applying,
        skill_id: skill_id.0.clone(),
        old_final_entity_path: old_entity.clone(),
        new_final_entity_path: new_entity.clone(),
        activations: vec![RelocateActivationStep {
            agent_id: "claude-code".into(),
            entry_path: entry_path.clone(),
            old_target_path: old_entity.clone(),
            new_target_path: new_entity.clone(),
            initial_entry: RelocateInitialEntry::Symlink {
                old_target: old_entity.clone(),
            },
        }],
    };
    let filesystem = MacOsFileSystem::new(harness.home.path().to_path_buf());
    filesystem
        .write_relocate_journal(&harness.library_root, &journal)
        .expect("write interrupted journal");

    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("startup recovery completes");

    // Rolled back: the entry is a dangling symlink to the old entity again
    // and the journal was archived.
    assert_eq!(
        std::fs::read_link(&entry_path).expect("rolled back Activation"),
        old_entity
    );
    assert!(
        !harness
            .library_root
            .join("operations/relocate-rollback-test/relocate-journal.json")
            .exists(),
        "journal must be archived after recovery"
    );
    assert!(
        harness
            .library_root
            .join("operation-history/relocate-rollback-test.relocate.json")
            .is_file(),
        "journal must be archived into operation history"
    );
    let detail = harness
        .catalog
        .inspect(skill_id)
        .expect("inspect rolled-back Link");
    assert_eq!(detail.summary.health, Health::Broken);
}

#[test]
fn interrupted_relocation_rolls_forward_at_startup_when_the_catalog_committed() {
    let harness = harness();
    let source = harness.home.path().join("Projects/forward-skill");
    let agent_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&agent_root).expect("create Agent directory");
    write_skill(&source, "forward-skill", "forward-skill", "Forward.");
    let skill_id = import_and_enable(&harness, &source, &["claude-code"]);
    let old_entity = canonical(&source);
    let entry_path = agent_root.join("forward-skill");
    std::fs::remove_dir_all(&source).expect("remove Link source");

    let replacement = harness.home.path().join("Projects/forward-skill-new");
    write_skill(&replacement, "forward-skill", "forward-skill", "Forward.");
    let new_entity = canonical(&replacement);

    // The catalog committed the new pointer, but the process died before any
    // Activation was rewritten.
    let activations = harness
        .runtime
        .activation_baselines_for_skill(&skill_id)
        .expect("desired Activations");
    harness
        .runtime
        .commit_relocate(
            &skill_id,
            new_entity.clone(),
            "forward-skill".into(),
            "Forward.".into(),
            new_entity.clone(),
            &activations,
        )
        .expect("commit relocation");
    let journal = RelocateJournal {
        version: 1,
        operation_id: "relocate-forward-test".into(),
        phase: RelocateJournalPhase::Applying,
        skill_id: skill_id.0.clone(),
        old_final_entity_path: old_entity.clone(),
        new_final_entity_path: new_entity.clone(),
        activations: vec![RelocateActivationStep {
            agent_id: "claude-code".into(),
            entry_path: entry_path.clone(),
            old_target_path: old_entity.clone(),
            new_target_path: new_entity.clone(),
            initial_entry: RelocateInitialEntry::Symlink {
                old_target: old_entity.clone(),
            },
        }],
    };
    let filesystem = MacOsFileSystem::new(harness.home.path().to_path_buf());
    filesystem
        .write_relocate_journal(&harness.library_root, &journal)
        .expect("write interrupted journal");

    let maintenance = harness.maintenance.clone().begin_startup();
    maintenance
        .run_activation_health_check()
        .expect("startup recovery completes");

    // Rolled forward: the Activation now points at the new entity and health
    // recovered.
    assert_eq!(
        std::fs::read_link(&entry_path).expect("forwarded Activation"),
        new_entity
    );
    assert!(
        !harness
            .library_root
            .join("operations/relocate-forward-test")
            .exists(),
        "journal must be archived after recovery"
    );
    let detail = harness
        .catalog
        .inspect(skill_id)
        .expect("inspect forwarded Link");
    assert_eq!(detail.summary.health, Health::Healthy);
    assert_eq!(detail.final_entity_path, new_entity.to_string_lossy());
}
