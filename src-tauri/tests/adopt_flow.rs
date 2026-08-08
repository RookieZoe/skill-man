//! End-to-end Adopt flows (ADR-0005): scanning and grouping by canonical
//! entity, real-directory migration, external Link registration, shared
//! splitting, per-Skill transactions with independent rollback, batch Undo
//! with occupancy re-checks, and journal recovery.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_man_lib::adapters::fixture_catalog::FixtureCatalogStore;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::adopt::{
    AdoptError, AdoptPlanKind, AdoptRisk, AdoptSelection, AdoptService,
};
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::domain::{CatalogFilter, Health, SourceKind};
use skill_man_lib::core::import::ImportService;
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::seams::import_store::ImportStore;

fn write_skill(directory: &Path, name: &str, body: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::create_dir_all(&path).expect("create skill directory");
    std::fs::write(path.join("SKILL.md"), body).expect("write SKILL.md");
    path
}

struct Harness {
    home: tempfile::TempDir,
    library_root: PathBuf,
    runtime: Arc<RuntimeCatalogStore>,
    filesystem: Arc<MacOsFileSystem>,
}

impl Harness {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("temporary home");
        let library_root = home.path().join("Library/Application Support/skill-man");
        let fixture = Arc::new(
            FixtureCatalogStore::runtime(&library_root).expect("materialize runtime fixture"),
        );
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
        Self {
            home,
            library_root,
            runtime,
            filesystem,
        }
    }

    fn adopt(&self) -> AdoptService {
        AdoptService::new(
            self.runtime.clone(),
            self.filesystem.clone(),
            Arc::new(SystemClock::new()),
            self.library_root.clone(),
            self.home.path().to_path_buf(),
        )
    }

    fn catalog(&self) -> CatalogService {
        CatalogService::new(self.runtime.clone())
    }

    fn claude_skills(&self) -> PathBuf {
        let path = self.home.path().join(".claude/skills");
        std::fs::create_dir_all(&path).expect("create Claude skills directory");
        path
    }

    fn codex_skills(&self) -> PathBuf {
        let path = self.home.path().join(".codex/skills");
        std::fs::create_dir_all(&path).expect("create Codex skills directory");
        path
    }

    fn shared_skills(&self) -> PathBuf {
        let path = self.home.path().join(".agents/skills");
        std::fs::create_dir_all(&path).expect("create shared skills directory");
        path
    }

    fn entity(&self, name: &str) -> PathBuf {
        self.library_root.join("skills").join(name)
    }
}

fn select(candidates: &[skill_man_lib::core::adopt::AdoptCandidate], name: &str) -> AdoptSelection {
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.directory_name == name)
        .unwrap_or_else(|| panic!("candidate '{name}' not found"));
    AdoptSelection {
        canonical_entity: candidate.canonical_entity.clone(),
        agent_ids: Vec::new(),
    }
}

#[test]
fn adopt_scan_groups_appearances_by_canonical_entity_and_marks_risks() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let codex = harness.codex_skills();
    let shared = harness.shared_skills();
    let real = write_skill(&claude, "foo", "# Foo\n");
    std::os::unix::fs::symlink(&real, codex.join("foo")).expect("symlink appearance");
    write_skill(&shared, "bar", "# Bar\n");
    write_skill(&claude, ".hidden", "# Hidden\n");
    write_skill(&codex, ".system", "# System\n");
    std::fs::write(claude.join("notes.txt"), "not a skill").expect("plain file");
    std::os::unix::fs::symlink(claude.join("missing-target"), claude.join("dangling-link"))
        .expect("dangling symlink");

    let candidates = harness.adopt().scan().expect("scan").candidates;
    let foo = candidates
        .iter()
        .find(|candidate| candidate.directory_name == "foo")
        .expect("foo candidate");
    assert_eq!(
        foo.canonical_entity,
        real.canonicalize().expect("canonical")
    );
    assert_eq!(foo.appearances.len(), 2);
    assert!(foo.adoptable);
    assert_eq!(foo.risk, AdoptRisk::None);
    assert_eq!(foo.directory_names, vec!["foo"]);

    let bar = candidates
        .iter()
        .find(|candidate| candidate.directory_name == "bar")
        .expect("bar candidate");
    assert!(
        bar.appearances.iter().all(|appearance| appearance.shared),
        "shared entries are flagged"
    );
    assert_eq!(
        bar.suggested_agent_ids.len(),
        2,
        "Claude Code and Codex read the shared directory"
    );

    let dangling = candidates
        .iter()
        .find(|candidate| candidate.directory_name == "dangling-link")
        .expect("dangling candidate");
    assert_eq!(dangling.risk, AdoptRisk::Broken);
    assert!(!dangling.adoptable);

    assert!(
        !candidates
            .iter()
            .any(|candidate| candidate.directory_name == ".hidden"),
        "dot-prefixed entries are excluded"
    );
    assert!(
        !candidates
            .iter()
            .any(|candidate| candidate.directory_name == ".system"),
        "Codex .system is excluded"
    );
    assert!(
        !candidates
            .iter()
            .any(|candidate| candidate.directory_name == "notes.txt"),
        "plain files are not candidates"
    );
}

#[test]
fn adopt_migrates_a_real_directory_and_replaces_the_appearance() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let source = write_skill(
        &claude,
        "foo",
        "---\nname: foo\ndescription: Adopted.\n---\n# Foo\n",
    );
    let canonical = source.canonicalize().expect("canonical source");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "foo")])
        .expect("plan Adopt");
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].kind, AdoptPlanKind::Migrate);
    assert_eq!(plan.items[0].final_entity_path, harness.entity("foo"));
    assert!(plan.can_apply);

    let result = adopt.apply(&plan.plan_token).expect("apply Adopt");
    assert!(result.items[0].adopted, "{:?}", result.items[0].error);
    assert!(result.undo_available);

    let entry = claude.join("foo");
    let target = std::fs::read_link(&entry).expect("appearance became an Activation");
    assert_eq!(target, harness.entity("foo"));
    assert_eq!(
        std::fs::read_to_string(harness.entity("foo").join("SKILL.md")).expect("entity content"),
        "---\nname: foo\ndescription: Adopted.\n---\n# Foo\n"
    );

    let installs = harness
        .catalog()
        .list(CatalogFilter::Install)
        .expect("list Installs")
        .items;
    let adopted = installs
        .iter()
        .find(|skill| skill.directory_name == "foo")
        .expect("adopted Skill is visible");
    assert_eq!(adopted.source_kind, SourceKind::FileInstall);
    let detail = harness
        .catalog()
        .inspect(adopted.id.clone())
        .expect("inspect adopted Skill");
    assert!(
        detail.source_label.contains("Installed from file"),
        "{}",
        detail.source_label
    );
    assert_eq!(detail.summary.health, Health::Healthy);

    let record = harness
        .runtime
        .load_file_install("foo")
        .expect("load file source")
        .expect("file source row");
    assert_eq!(record.original_path, canonical);
    assert_eq!(record.library_entry_path, harness.entity("foo"));

    let journal = harness
        .library_root
        .join("operations")
        .read_dir()
        .expect("operations")
        .filter_map(Result::ok)
        .find(|entry| entry.path().join("adopt-journal.json").is_file())
        .expect("adopt journal persists until the result window closes");
    adopt
        .finalize(&result.operation_id)
        .expect("finalize closes the window");
    assert!(
        !journal.path().join("adopt-journal.json").exists(),
        "finalize archives the journal"
    );
}

#[test]
fn adopt_registers_external_symlinks_as_links() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let projects = harness.home.path().join("Projects");
    let external = write_skill(&projects, "ext-foo", "# External\n");
    std::os::unix::fs::symlink(&external, claude.join("ext-foo")).expect("symlink appearance");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "ext-foo")])
        .expect("plan Adopt");
    assert_eq!(plan.items[0].kind, AdoptPlanKind::Link);
    assert_eq!(
        plan.items[0].final_entity_path,
        external.canonicalize().expect("canonical external")
    );
    let result = adopt.apply(&plan.plan_token).expect("apply Adopt");
    assert!(result.items[0].adopted, "{:?}", result.items[0].error);

    assert!(
        external.join("SKILL.md").is_file(),
        "external entities stay in place for Links"
    );
    let target = std::fs::read_link(claude.join("ext-foo")).expect("Activation target");
    assert_eq!(target, external.canonicalize().expect("canonical"));
    let links = harness
        .catalog()
        .list(CatalogFilter::Link)
        .expect("list Links")
        .items;
    assert!(
        links.iter().any(|skill| skill.directory_name == "ext-foo"),
        "the Link registration is visible in the catalog"
    );
}

#[test]
fn adopt_splits_shared_entries_into_per_agent_activations() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let codex = harness.codex_skills();
    let shared = harness.shared_skills();
    write_skill(&shared, "shared-tool", "# Shared\n");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "shared-tool")])
        .expect("plan Adopt");
    assert_eq!(
        plan.items[0].target_agents.len(),
        2,
        "both detected readers are pre-selected"
    );
    let result = adopt.apply(&plan.plan_token).expect("apply Adopt");
    assert!(result.items[0].adopted, "{:?}", result.items[0].error);

    assert!(
        !shared.join("shared-tool").exists(),
        "the shared entry is removed"
    );
    let claude_target = std::fs::read_link(claude.join("shared-tool")).expect("Claude Activation");
    assert_eq!(claude_target, harness.entity("shared-tool"));
    let codex_target = std::fs::read_link(codex.join("shared-tool")).expect("Codex Activation");
    assert_eq!(codex_target, harness.entity("shared-tool"));
}

#[test]
fn adopt_partial_failure_rolls_back_only_the_failed_skill() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let shared = harness.shared_skills();
    write_skill(&claude, "alpha", "# Alpha\n");
    write_skill(&shared, "beta", "# Beta\n");
    // Occupy the Claude-side Activation entry for the shared beta item.
    write_skill(&claude, "beta", "# Occupied\n");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "alpha"), select(&candidates, "beta")])
        .expect("plan Adopt");
    assert_eq!(plan.items.len(), 2);
    let result = adopt.apply(&plan.plan_token).expect("apply Adopt");

    let alpha = result
        .items
        .iter()
        .find(|item| item.directory_name == "alpha")
        .expect("alpha result");
    assert!(alpha.adopted, "{:?}", alpha.error);
    let beta = result
        .items
        .iter()
        .find(|item| item.directory_name == "beta")
        .expect("beta result");
    assert!(!beta.adopted);
    let beta_error = beta.error.as_deref().expect("beta failure reason");
    assert!(beta_error.contains("occupied"), "{beta_error}");

    assert!(
        shared.join("beta").join("SKILL.md").is_file(),
        "the failed Skill was restored to its original shared entry"
    );
    assert!(
        !harness.entity("beta").exists(),
        "no Library entity remains for the failed Skill"
    );
    assert_eq!(
        std::fs::read_to_string(claude.join("beta/SKILL.md")).expect("occupying entry"),
        "# Occupied\n"
    );
    let installs = harness
        .catalog()
        .list(CatalogFilter::Install)
        .expect("list Installs")
        .items;
    assert!(
        !installs.iter().any(|skill| skill.directory_name == "beta"),
        "the failed Skill has no catalog row"
    );
    assert!(
        installs.iter().any(|skill| skill.directory_name == "alpha"),
        "the successful Skill stays"
    );
}

#[test]
fn adopt_undo_restores_original_locations_and_rows() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let source = write_skill(&claude, "foo", "# Foo\n");
    let canonical = source.canonicalize().expect("canonical source");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "foo")])
        .expect("plan Adopt");
    let result = adopt.apply(&plan.plan_token).expect("apply Adopt");

    let undo = adopt.undo(&result.operation_id).expect("undo Adopt");
    assert!(undo.items[0].undone, "{:?}", undo.items[0].error);
    assert!(
        claude.join("foo").canonicalize().expect("restored") == canonical,
        "the entity moved back to its original location"
    );
    assert!(
        std::fs::symlink_metadata(claude.join("foo"))
            .expect("restored entry")
            .file_type()
            .is_dir(),
        "the restored entry is a real directory again"
    );
    assert!(!harness.entity("foo").exists());
    let installs = harness
        .catalog()
        .list(CatalogFilter::Install)
        .expect("list Installs")
        .items;
    assert!(
        !installs.iter().any(|skill| skill.directory_name == "foo"),
        "the catalog row is gone"
    );
    let again = adopt
        .undo(&result.operation_id)
        .expect_err("the window closed after the first Undo");
    assert!(matches!(again, AdoptError::PlanNotFound));
}

#[test]
fn adopt_undo_skips_entries_occupied_since_apply() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    write_skill(&claude, "foo", "# Foo\n");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "foo")])
        .expect("plan Adopt");
    let result = adopt.apply(&plan.plan_token).expect("apply Adopt");

    std::fs::remove_file(claude.join("foo")).expect("remove Activation before occupying");
    write_skill(&claude, "foo", "# New occupant\n");
    let undo = adopt.undo(&result.operation_id).expect("undo Adopt");
    assert!(
        !undo.items[0].undone,
        "occupied original paths are never overwritten"
    );
    let error = undo.items[0].error.as_deref().expect("undo failure");
    assert!(
        error.contains("occupied") || error.contains("exists") || error.contains("changed"),
        "{error}"
    );
    assert!(
        harness.entity("foo").join("SKILL.md").is_file(),
        "the Library entity is untouched"
    );
}

#[test]
fn adopt_conflicts_with_managed_identity_are_not_adoptable() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    write_skill(&claude, "conflicted", "# Untracked\n");

    let import = ImportService::new(
        harness.runtime.clone(),
        harness.filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(skill_man_lib::adapters::local_file_source::LocalFileSource::new()),
        harness.library_root.clone(),
    );
    let managed_source = harness.home.path().join("Projects/conflicted");
    write_skill(&managed_source, "", "# Managed\n");
    let managed_entity = managed_source.join("conflicted");
    std::fs::create_dir_all(&managed_entity).expect("managed entity");
    std::fs::write(managed_entity.join("SKILL.md"), "# Managed\n").expect("managed SKILL.md");
    import
        .discover_link(&managed_entity)
        .expect("discover managed Link");
    let link_preview = import.plan_link(&managed_entity).expect("plan Link");
    import
        .apply_link(&link_preview.plan_token)
        .expect("apply Link");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let conflicted = candidates
        .iter()
        .find(|candidate| candidate.directory_name == "conflicted")
        .expect("conflicted candidate");
    assert!(!conflicted.adoptable);
    assert!(conflicted.conflict.is_some());
    let plan = adopt
        .plan(&[select(&candidates, "conflicted")])
        .expect("plan conflicted");
    assert!(!plan.can_apply);
    assert!(
        plan.items[0]
            .error
            .as_deref()
            .expect("conflict error")
            .contains("already uses"),
        "{:?}",
        plan.items[0].error
    );
}

#[test]
fn adopt_recovery_discards_uncommitted_entities_and_forwards_committed_items() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let filesystem = harness.filesystem.clone();
    let library_root = harness.library_root.clone();

    // Item 1: entity installed, catalog row absent → recovery discards it.
    let ghost = harness.entity("ghost");
    std::fs::create_dir_all(&ghost).expect("ghost entity");
    std::fs::write(ghost.join("SKILL.md"), "# Ghost\n").expect("ghost SKILL.md");
    let ghost_fingerprint = filesystem
        .directory_fingerprint(&ghost)
        .expect("fingerprint");

    // Item 2: entity installed AND catalog row committed → recovery forwards
    // and creates the planned Activation.
    let committed = harness.entity("committed");
    std::fs::create_dir_all(&committed).expect("committed entity");
    std::fs::write(committed.join("SKILL.md"), "# Committed\n").expect("committed SKILL.md");
    let committed_fingerprint = filesystem
        .directory_fingerprint(&committed)
        .expect("fingerprint");
    let committed_hash = filesystem.tree_hash(&committed).expect("hash");
    harness
        .runtime
        .insert_file(skill_man_lib::seams::import_store::FileImportRecord {
            skill_id: skill_man_lib::core::domain::SkillId("committed-skill".into()),
            directory_name: "committed".into(),
            identity_key: "committed".into(),
            display_name: "committed".into(),
            description: String::new(),
            library_entry_path: committed.clone(),
            final_entity_path: committed.clone(),
            recorded_content_hash: committed_hash.clone(),
            original_path: claude.join("committed"),
            original_filename: "committed".into(),
        })
        .expect("insert committed row");

    let operation_id = "adopt-crash-test";
    let staging_root = library_root.join("staging").join(operation_id);
    let journal = skill_man_lib::seams::filesystem::AdoptJournal {
        version: 1,
        operation_id: operation_id.into(),
        phase: skill_man_lib::seams::filesystem::AdoptJournalPhase::Applying,
        staging_operation_root: staging_root.clone(),
        staging_fingerprint: skill_man_lib::seams::filesystem::DirectoryFingerprint {
            canonical_path: staging_root.clone(),
            device: 0,
            inode: 0,
        },
        items: vec![
            skill_man_lib::seams::filesystem::AdoptJournalItem {
                skill_id: "ghost-skill".into(),
                directory_name: "ghost".into(),
                kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Migrate,
                staged_root: PathBuf::new(),
                staged_fingerprint: skill_man_lib::seams::filesystem::DirectoryFingerprint {
                    canonical_path: PathBuf::new(),
                    device: 0,
                    inode: 0,
                },
                final_entity_path: ghost.clone(),
                recorded_content_hash: "ghost-hash".into(),
                original_path: claude.join("ghost"),
                original_filename: "ghost".into(),
                appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
                    entry_path: claude.join("ghost"),
                    kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::RealDirectory,
                }],
                activations: vec![],
                phase: skill_man_lib::seams::filesystem::AdoptItemPhase::EntityInstalled,
                installed_fingerprint: Some(ghost_fingerprint),
            },
            skill_man_lib::seams::filesystem::AdoptJournalItem {
                skill_id: "committed-skill".into(),
                directory_name: "committed".into(),
                kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Migrate,
                staged_root: PathBuf::new(),
                staged_fingerprint: skill_man_lib::seams::filesystem::DirectoryFingerprint {
                    canonical_path: PathBuf::new(),
                    device: 0,
                    inode: 0,
                },
                final_entity_path: committed.clone(),
                recorded_content_hash: committed_hash.clone(),
                original_path: claude.join("committed"),
                original_filename: "committed".into(),
                appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
                    entry_path: claude.join("committed"),
                    kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::RealDirectory,
                }],
                activations: vec![skill_man_lib::seams::filesystem::AdoptActivationStep {
                    agent_id: "claude-code".into(),
                    entry_path: claude.join("committed"),
                    target_path: committed.clone(),
                }],
                phase: skill_man_lib::seams::filesystem::AdoptItemPhase::CatalogCommitted,
                installed_fingerprint: Some(committed_fingerprint),
            },
        ],
    };
    filesystem
        .write_adopt_journal(&library_root, &journal)
        .expect("write crash journal");

    filesystem
        .recover_adopt_journals(
            &library_root,
            &[
                skill_man_lib::seams::filesystem::FileImportRecoveryBaseline {
                    skill_id: "committed-skill".into(),
                    final_entity_path: committed.clone(),
                    recorded_content_hash: committed_hash.clone(),
                },
            ],
            &[
                skill_man_lib::seams::filesystem::FileImportRecoveryBaseline {
                    skill_id: "committed-skill".into(),
                    final_entity_path: committed.clone(),
                    recorded_content_hash: committed_hash,
                },
            ],
        )
        .expect("recover adopt journals");

    assert!(
        claude.join("ghost").join("SKILL.md").is_file(),
        "the uncommitted entity was restored to its original location during recovery"
    );
    assert!(!ghost.exists(), "nothing remains at the Library path");
    assert!(
        std::fs::read_link(claude.join("committed")).expect("forwarded Activation") == committed,
        "the committed item forwarded to its Activation"
    );
    assert!(
        !library_root.join("operations").join(operation_id).exists(),
        "the recovered journal was archived"
    );
}
