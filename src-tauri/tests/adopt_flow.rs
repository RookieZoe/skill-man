//! End-to-end Adopt flows (ADR-0005): scanning and grouping by canonical
//! entity, real-directory migration, external Link registration, shared
//! splitting, per-Skill transactions with independent rollback, batch Undo
//! with occupancy re-checks, and journal recovery.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use skill_man_lib::adapters::fixture_catalog::FixtureCatalogStore;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::adopt::{
    AdoptError, AdoptPlanKind, AdoptRisk, AdoptSelection, AdoptService,
};
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::domain::{AgentId, CatalogFilter, Health, SkillId, SourceKind};
use skill_man_lib::core::import::ImportService;
use skill_man_lib::core::maintenance::MaintenanceService;
use skill_man_lib::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedSkillRecord, LibraryConflict,
};
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::seams::import_store::ImportStore;
use skill_man_lib::seams::recovery::RecoveryGate;

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

struct FixedClock;

impl Clock for FixedClock {
    fn monotonic_millis(&self) -> u128 {
        10
    }

    fn unix_epoch_nanos(&self) -> u128 {
        42
    }
}

struct FailDoneJournalFileSystem {
    delegate: MacOsFileSystem,
    failed: AtomicBool,
}

struct FailFirstAdoptRemovalStore {
    delegate: Arc<RuntimeCatalogStore>,
    failed: AtomicBool,
}

impl FailFirstAdoptRemovalStore {
    fn new(delegate: Arc<RuntimeCatalogStore>) -> Self {
        Self {
            delegate,
            failed: AtomicBool::new(false),
        }
    }
}

impl AdoptStore for FailFirstAdoptRemovalStore {
    fn list_agents(&self) -> Result<Vec<AdoptAgent>, AdoptStoreError> {
        AdoptStore::list_agents(self.delegate.as_ref())
    }

    fn mark_agent_detected(&self, agent_id: &AgentId) -> Result<(), AdoptStoreError> {
        AdoptStore::mark_agent_detected(self.delegate.as_ref(), agent_id)
    }

    fn insert_adopted(&self, record: AdoptedSkillRecord) -> Result<u64, AdoptStoreError> {
        AdoptStore::insert_adopted(self.delegate.as_ref(), record)
    }

    fn remove_adopted_skill(&self, skill_id: &SkillId) -> Result<u64, AdoptStoreError> {
        if !self.failed.swap(true, Ordering::SeqCst) {
            return Err(AdoptStoreError::Unavailable(
                "injected Adopt catalog removal failure".into(),
            ));
        }
        AdoptStore::remove_adopted_skill(self.delegate.as_ref(), skill_id)
    }

    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<LibraryConflict>, AdoptStoreError> {
        AdoptStore::find_library_conflict(self.delegate.as_ref(), identity_key)
    }
}

impl FailDoneJournalFileSystem {
    fn new(home_directory: PathBuf) -> Self {
        Self {
            delegate: MacOsFileSystem::new(home_directory),
            failed: AtomicBool::new(false),
        }
    }
}

impl FileSystem for FailDoneJournalFileSystem {
    fn inspect_link_source(
        &self,
        path: &Path,
    ) -> Result<
        skill_man_lib::seams::filesystem::LinkSourceSnapshot,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate.inspect_link_source(path)
    }

    fn canonical_directory(
        &self,
        path: &Path,
    ) -> Result<PathBuf, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.canonical_directory(path)
    }

    fn normalize_configured_path(
        &self,
        path: &Path,
    ) -> Result<PathBuf, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.normalize_configured_path(path)
    }

    fn directory_fingerprint(
        &self,
        path: &Path,
    ) -> Result<
        skill_man_lib::seams::filesystem::DirectoryFingerprint,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate.directory_fingerprint(path)
    }

    fn activation_snapshot(
        &self,
        entry_path: &Path,
    ) -> Result<
        skill_man_lib::seams::filesystem::ActivationEntrySnapshot,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate.activation_snapshot(entry_path)
    }

    fn skill_directory_is_readable(
        &self,
        path: &Path,
    ) -> Result<bool, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.skill_directory_is_readable(path)
    }

    fn skill_fingerprint(
        &self,
        path: &Path,
    ) -> Result<
        skill_man_lib::seams::filesystem::SkillFingerprint,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate.skill_fingerprint(path)
    }

    fn read_skill_document(
        &self,
        path: &Path,
    ) -> Result<String, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.read_skill_document(path)
    }

    fn tree_hash(
        &self,
        path: &Path,
    ) -> Result<String, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.tree_hash(path)
    }

    fn staged_tree_snapshot(
        &self,
        path: &Path,
    ) -> Result<
        skill_man_lib::seams::filesystem::StagedTreeSnapshot,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate.staged_tree_snapshot(path)
    }

    fn available_space(
        &self,
        path: &Path,
    ) -> Result<u64, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.available_space(path)
    }

    fn staged_child_directories(
        &self,
        path: &Path,
    ) -> Result<Vec<PathBuf>, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.staged_child_directories(path)
    }

    fn staged_has_skill_document(
        &self,
        directory: &Path,
        filename: &str,
    ) -> Result<bool, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.staged_has_skill_document(directory, filename)
    }

    fn canonicalize_staged_path(
        &self,
        path: &Path,
    ) -> Result<PathBuf, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.canonicalize_staged_path(path)
    }

    fn install_staged_skill(
        &self,
        staged_skill_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected_staged_tree: &skill_man_lib::seams::filesystem::StagedTreeSnapshot,
    ) -> Result<
        skill_man_lib::seams::filesystem::DirectoryFingerprint,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate.install_staged_skill(
            staged_skill_path,
            final_entity_path,
            library_root,
            operation_id,
            expected_staged_tree,
        )
    }

    fn discard_staging(
        &self,
        staging_operation_root: &Path,
        library_root: &Path,
        expected: Option<&skill_man_lib::seams::filesystem::DirectoryFingerprint>,
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate
            .discard_staging(staging_operation_root, library_root, expected)
    }

    fn discard_installed_skill(
        &self,
        final_entity_path: &Path,
        library_root: &Path,
        expected: &skill_man_lib::seams::filesystem::DirectoryFingerprint,
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate
            .discard_installed_skill(final_entity_path, library_root, expected)
    }

    fn create_activation(
        &self,
        target_path: &Path,
        entry_path: &Path,
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.create_activation(target_path, entry_path)
    }

    fn remove_activation(
        &self,
        entry_path: &Path,
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate.remove_activation(entry_path)
    }

    fn scan_skills_directory(
        &self,
        path: &Path,
    ) -> Result<
        Vec<skill_man_lib::seams::filesystem::ScannedSkillEntry>,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate.scan_skills_directory(path)
    }

    fn stage_external_directory(
        &self,
        source: &Path,
        staging_destination: &Path,
    ) -> Result<
        skill_man_lib::seams::filesystem::DirectoryFingerprint,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate
            .stage_external_directory(source, staging_destination)
    }

    fn create_adopt_staging_operation(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<
        skill_man_lib::seams::filesystem::DirectoryFingerprint,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate
            .create_adopt_staging_operation(library_root, operation_id)
    }

    fn stage_external_directory_in_adopt_operation(
        &self,
        source: &Path,
        library_root: &Path,
        operation_id: &str,
        directory_name: &str,
        expected_operation_root: &skill_man_lib::seams::filesystem::DirectoryFingerprint,
        expected_source: &skill_man_lib::seams::filesystem::DirectoryFingerprint,
    ) -> Result<
        skill_man_lib::seams::filesystem::DirectoryFingerprint,
        skill_man_lib::seams::filesystem::FileSystemError,
    > {
        self.delegate.stage_external_directory_in_adopt_operation(
            source,
            library_root,
            operation_id,
            directory_name,
            expected_operation_root,
            expected_source,
        )
    }

    fn discard_isolated_adopt_source(
        &self,
        source: &Path,
        operation_id: &str,
        expected_source: &skill_man_lib::seams::filesystem::DirectoryFingerprint,
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate
            .discard_isolated_adopt_source(source, operation_id, expected_source)
    }

    fn restore_external_directory(
        &self,
        source: &Path,
        destination: &Path,
        expected: &skill_man_lib::seams::filesystem::DirectoryFingerprint,
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate
            .restore_external_directory(source, destination, expected)
    }

    fn apply_adopt_appearances(
        &self,
        appearances: &[skill_man_lib::seams::filesystem::AdoptAppearanceStep],
        activations: &[skill_man_lib::seams::filesystem::AdoptActivationStep],
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate
            .apply_adopt_appearances(appearances, activations)
    }

    fn write_adopt_journal(
        &self,
        library_root: &Path,
        journal: &skill_man_lib::seams::filesystem::AdoptJournal,
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        if journal
            .items
            .iter()
            .any(|item| item.phase == skill_man_lib::seams::filesystem::AdoptItemPhase::Done)
            && !self.failed.swap(true, Ordering::SeqCst)
        {
            return Err(skill_man_lib::seams::filesystem::FileSystemError::Io {
                operation: "write injected Done Adopt journal",
                path: library_root
                    .join("operations")
                    .join(&journal.operation_id)
                    .join("adopt-journal.json"),
                source: std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected Done journal failure",
                ),
            });
        }
        self.delegate.write_adopt_journal(library_root, journal)
    }

    fn finish_adopt_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate
            .finish_adopt_journal(library_root, operation_id)
    }

    fn recover_adopt_journals(
        &self,
        library_root: &Path,
        baselines: &[skill_man_lib::seams::filesystem::FileImportRecoveryBaseline],
        adopted_entities: &[skill_man_lib::seams::filesystem::FileImportRecoveryBaseline],
    ) -> Result<u32, skill_man_lib::seams::filesystem::FileSystemError> {
        self.delegate
            .recover_adopt_journals(library_root, baselines, adopted_entities)
    }
}

struct StagedAdoptFixture {
    source: PathBuf,
    staging_operation_root: PathBuf,
    staged_root: PathBuf,
    journal: skill_man_lib::seams::filesystem::AdoptJournal,
}

fn staged_adopt_fixture(
    harness: &Harness,
    operation_id: &str,
    directory_name: &str,
) -> StagedAdoptFixture {
    let source = write_skill(
        &harness.shared_skills(),
        directory_name,
        &format!("# {directory_name}\n"),
    );
    let staging_operation_root = harness.library_root.join("staging").join(operation_id);
    let staged_root = staging_operation_root.join(directory_name);
    let staged_fingerprint = harness
        .filesystem
        .stage_external_directory(&source, &staged_root)
        .expect("stage Adopt fixture");
    let staging_fingerprint = harness
        .filesystem
        .directory_fingerprint(&staging_operation_root)
        .expect("fingerprint fixture staging root");
    let recorded_content_hash = harness
        .filesystem
        .tree_hash(&staged_root)
        .expect("hash staged fixture");
    let journal = skill_man_lib::seams::filesystem::AdoptJournal {
        version: 2,
        operation_id: operation_id.into(),
        phase: skill_man_lib::seams::filesystem::AdoptJournalPhase::Applying,
        staging_operation_root: staging_operation_root.clone(),
        staging_fingerprint,
        items: vec![skill_man_lib::seams::filesystem::AdoptJournalItem {
            skill_id: format!("{directory_name}-skill"),
            directory_name: directory_name.into(),
            kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Migrate,
            staged_root: staged_root.clone(),
            source_fingerprint: None,
            staged_fingerprint,
            final_entity_path: harness.entity(directory_name),
            recorded_content_hash,
            original_path: source.clone(),
            original_filename: directory_name.into(),
            appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
                entry_path: source.clone(),
                kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::RealDirectory,
            }],
            activations: Vec::new(),
            phase: skill_man_lib::seams::filesystem::AdoptItemPhase::Staged,
            installed_fingerprint: None,
        }],
    };
    StagedAdoptFixture {
        source,
        staging_operation_root,
        staged_root,
        journal,
    }
}

fn insert_committed_migrate_row(
    harness: &Harness,
    item: &skill_man_lib::seams::filesystem::AdoptJournalItem,
    original_path: &Path,
) {
    harness
        .runtime
        .insert_file(skill_man_lib::seams::import_store::FileImportRecord {
            skill_id: SkillId(item.skill_id.clone()),
            directory_name: item.directory_name.clone(),
            identity_key: item.directory_name.clone(),
            display_name: item.directory_name.clone(),
            description: String::new(),
            library_entry_path: item.final_entity_path.clone(),
            final_entity_path: item.final_entity_path.clone(),
            recorded_content_hash: item.recorded_content_hash.clone(),
            original_path: original_path.to_path_buf(),
            original_filename: item.original_filename.clone(),
        })
        .expect("insert committed Adopt row");
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
fn adopt_preview_rejects_an_unsafe_skill_without_moving_the_source() {
    let harness = Harness::new();
    let shared = harness.shared_skills();
    let source = write_skill(&shared, "ask-matt", "# Ask Matt\n");
    std::os::unix::fs::symlink(&source, source.join("ask-matt"))
        .expect("create the installer-style absolute self-link");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let ask_matt = candidates
        .iter()
        .find(|candidate| candidate.directory_name == "ask-matt")
        .expect("ask-matt candidate");
    assert!(!ask_matt.adoptable);
    assert_eq!(ask_matt.risk, AdoptRisk::Broken);
    assert_eq!(
        ask_matt.risk_reason.as_deref(),
        Some("Skill symlink target must be a valid UTF-8 relative path: ask-matt")
    );
    let plan = adopt
        .plan(&[select(&candidates, "ask-matt")])
        .expect("the unsafe Skill is represented as a blocked Preview item");
    assert!(!plan.can_apply);
    assert!(
        source.join("SKILL.md").is_file(),
        "Preview must leave the Untracked Skill at its original location"
    );
    assert_eq!(
        std::fs::read_link(source.join("ask-matt")).expect("self-link remains in place"),
        source,
        "Preview must not rewrite the source tree"
    );
    assert!(
        !harness.library_root.join("staging").exists()
            || harness
                .library_root
                .join("staging")
                .read_dir()
                .expect("read staging")
                .next()
                .is_none(),
        "a rejected Preview must not leave staging content"
    );
}

#[test]
fn adopt_preview_and_cancel_leave_a_migrating_skill_unchanged() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let source = write_skill(&claude, "preview-only", "# Preview only\n");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "preview-only")])
        .expect("Preview Adopt");

    assert!(plan.can_apply);
    assert!(
        source.join("SKILL.md").is_file(),
        "Preview must not move a valid Untracked Skill"
    );
    assert!(
        !harness.entity("preview-only").exists(),
        "Preview must not install an entity"
    );
    assert!(
        !harness.library_root.join("staging").exists(),
        "Preview must not create staging"
    );
    assert!(
        !harness.library_root.join("operations").exists(),
        "Preview must not write a journal"
    );

    assert!(adopt.cancel(&plan.plan_token).expect("cancel Preview"));
    assert!(source.join("SKILL.md").is_file());
    assert!(!harness.entity("preview-only").exists());
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
fn adopt_apply_rejects_a_source_changed_after_preview_without_writing() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let source = write_skill(&claude, "changed-after-preview", "# Original\n");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "changed-after-preview")])
        .expect("Preview Adopt");
    std::fs::write(source.join("SKILL.md"), "# Changed\n").expect("change source after Preview");

    let error = adopt
        .apply(&plan.plan_token)
        .expect_err("a changed Preview source must make the plan stale");

    assert!(matches!(error, AdoptError::PlanStale));
    assert_eq!(
        std::fs::read_to_string(source.join("SKILL.md")).expect("source remains"),
        "# Changed\n"
    );
    assert!(!harness.entity("changed-after-preview").exists());
    assert!(!harness.library_root.join("staging").exists());
    assert!(!harness.library_root.join("operations").exists());
}

#[test]
fn adopt_apply_never_follows_a_symlinked_staging_root() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let source = write_skill(&claude, "symlinked-staging", "# Symlinked staging\n");
    let recovery_gate = Arc::new(RecoveryGate::ready());
    let adopt = AdoptService::new(
        harness.runtime.clone(),
        harness.filesystem.clone(),
        Arc::new(FixedClock),
        harness.library_root.clone(),
        harness.home.path().to_path_buf(),
    )
    .with_recovery_gate(recovery_gate.clone());
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "symlinked-staging")])
        .expect("Preview Adopt");

    let outside = harness.home.path().join("must-not-stage-here");
    std::fs::create_dir_all(&outside).expect("create outside directory");
    std::os::unix::fs::symlink(&outside, harness.library_root.join("staging"))
        .expect("replace staging root with an external symlink");

    let error = adopt
        .apply(&plan.plan_token)
        .expect_err("Apply must refuse a symlinked staging root");

    assert!(matches!(error, AdoptError::RecoveryRequired(_)), "{error}");
    assert!(!recovery_gate.writes_are_ready());
    assert_eq!(
        std::fs::read_to_string(source.join("SKILL.md")).expect("source remains readable"),
        "# Symlinked staging\n"
    );
    assert!(
        std::fs::symlink_metadata(&source)
            .expect("source remains in place")
            .is_dir()
    );
    assert!(
        outside
            .read_dir()
            .expect("read outside directory")
            .next()
            .is_none(),
        "Apply must not create or move Adopt content through the staging symlink"
    );
    assert!(
        harness
            .library_root
            .join("operations/adopt-42-1/adopt-journal.json")
            .is_file(),
        "the durable intent remains for startup recovery"
    );

    let sentinel = outside.join("external-content.txt");
    std::fs::write(&sentinel, "must survive recovery").expect("write external sentinel");
    let restart_gate = Arc::new(RecoveryGate::blocked());
    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .with_recovery_gate(restart_gate.clone())
        .startup_check()
        .expect_err("startup must also refuse the symlinked staging root");
    assert!(!restart_gate.writes_are_ready());
    assert_eq!(
        std::fs::read_to_string(&sentinel).expect("external sentinel remains"),
        "must survive recovery",
        "recovery must never clean through the staging symlink"
    );
    assert!(source.join("SKILL.md").is_file());
    assert!(
        harness
            .library_root
            .join("operations/adopt-42-1/adopt-journal.json")
            .is_file(),
        "the unresolved journal remains fail-closed"
    );
}

#[test]
fn adopt_apply_blocks_further_writes_when_a_durable_intent_needs_recovery() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    write_skill(&claude, "stable-adopt", "# Stable Adopt\n");
    write_skill(&claude, "blocked-apply", "# Blocked apply\n");
    let recovery_gate = Arc::new(RecoveryGate::ready());
    let adopt = AdoptService::new(
        harness.runtime.clone(),
        harness.filesystem.clone(),
        Arc::new(FixedClock),
        harness.library_root.clone(),
        harness.home.path().to_path_buf(),
    )
    .with_recovery_gate(recovery_gate.clone());
    let candidates = adopt.scan().expect("scan").candidates;
    let stable_plan = adopt
        .plan(&[select(&candidates, "stable-adopt")])
        .expect("Preview stable Adopt");
    let stable_result = adopt
        .apply(&stable_plan.plan_token)
        .expect("apply stable Adopt");
    let plan = adopt
        .plan(&[select(&candidates, "blocked-apply")])
        .expect("Preview Adopt");

    let occupied_staging_root = harness.library_root.join("staging/adopt-42-3");
    std::fs::create_dir_all(occupied_staging_root.parent().expect("staging parent"))
        .expect("create staging parent");
    std::fs::write(&occupied_staging_root, "occupied")
        .expect("occupy the planned staging directory");

    let error = adopt
        .apply(&plan.plan_token)
        .expect_err("Apply must require startup recovery");

    assert!(matches!(error, AdoptError::RecoveryRequired(_)), "{error}");
    assert!(!recovery_gate.writes_are_ready());
    let second = adopt
        .plan(&[select(&candidates, "blocked-apply")])
        .expect_err("the session stays write-locked");
    assert!(matches!(second, AdoptError::RecoveryRequired(_)));
    let undo = adopt
        .undo(&stable_result.operation_id)
        .expect_err("Undo is also write-locked");
    assert!(matches!(undo, AdoptError::RecoveryRequired(_)));
    let finalize = adopt
        .finalize(&stable_result.operation_id)
        .expect_err("Finalize is also write-locked");
    assert!(matches!(finalize, AdoptError::RecoveryRequired(_)));
    assert!(harness.entity("stable-adopt/SKILL.md").is_file());
    assert_eq!(
        std::fs::read_link(claude.join("stable-adopt")).expect("stable Activation remains"),
        harness.entity("stable-adopt")
    );
    assert!(claude.join("blocked-apply/SKILL.md").is_file());
    assert!(
        harness
            .library_root
            .join("operations/adopt-42-3/adopt-journal.json")
            .is_file(),
        "the durable intent remains for startup recovery"
    );
}

#[test]
fn adopt_done_journal_failure_compensates_the_committed_item() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let source = write_skill(&claude, "done-write-failure", "# Done write failure\n");
    let filesystem = Arc::new(FailDoneJournalFileSystem::new(
        harness.home.path().to_path_buf(),
    ));
    let recovery_gate = Arc::new(RecoveryGate::ready());
    let adopt = AdoptService::new(
        harness.runtime.clone(),
        filesystem.clone(),
        Arc::new(SystemClock::new()),
        harness.library_root.clone(),
        harness.home.path().to_path_buf(),
    )
    .with_recovery_gate(recovery_gate.clone());
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "done-write-failure")])
        .expect("Preview Adopt");

    let result = adopt
        .apply(&plan.plan_token)
        .expect("the injected item failure is compensated inside the batch");

    assert!(filesystem.failed.load(Ordering::SeqCst));
    assert!(!result.items[0].adopted);
    assert!(
        result.items[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("injected Done journal failure")),
        "{:?}",
        result.items[0].error
    );
    assert!(!result.undo_available);
    assert!(recovery_gate.writes_are_ready());
    assert!(source.join("SKILL.md").is_file());
    assert!(
        std::fs::symlink_metadata(&source)
            .expect("restored source")
            .is_dir(),
        "the original appearance is restored as a real directory"
    );
    assert!(!harness.entity("done-write-failure").exists());
    assert!(
        !harness
            .catalog()
            .list(CatalogFilter::Install)
            .expect("list Installs")
            .items
            .iter()
            .any(|skill| skill.directory_name == "done-write-failure"),
        "the committed catalog row is removed during compensation"
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

    let undo = adopt
        .undo(&result.operation_id)
        .expect("Undo successful items");
    assert_eq!(undo.items.len(), 1);
    assert_eq!(undo.items[0].directory_name, "alpha");
    assert!(undo.items[0].undone, "{:?}", undo.items[0].error);
    assert!(
        shared.join("beta/SKILL.md").is_file(),
        "Undo must not revisit an item that already rolled back"
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
    let codex = harness.codex_skills();
    let shared = harness.shared_skills();
    write_skill(&shared, "foo", "# Foo\n");

    let adopt = harness.adopt();
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "foo")])
        .expect("plan Adopt");
    let result = adopt.apply(&plan.plan_token).expect("apply Adopt");
    let library_entity = harness.entity("foo");
    assert_eq!(
        std::fs::read_link(claude.join("foo")).expect("Claude Activation"),
        library_entity
    );
    assert_eq!(
        std::fs::read_link(codex.join("foo")).expect("Codex Activation"),
        library_entity
    );

    write_skill(&shared, "foo", "# New occupant\n");
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
        library_entity.join("SKILL.md").is_file(),
        "the Library entity is untouched"
    );
    assert_eq!(
        std::fs::read_to_string(shared.join("foo/SKILL.md")).expect("external occupant"),
        "# New occupant\n",
        "Undo must not overwrite the external occupant"
    );
    assert_eq!(
        std::fs::read_link(claude.join("foo")).expect("Claude Activation remains"),
        library_entity,
        "an occupied original path must be detected before removing any Activation"
    );
    assert_eq!(
        std::fs::read_link(codex.join("foo")).expect("Codex Activation remains"),
        library_entity,
        "the whole skipped item must remain unchanged"
    );
    assert!(
        harness
            .runtime
            .load_file_install("foo")
            .expect("load adopted source")
            .is_some(),
        "the catalog row remains for a skipped Undo item"
    );
}

#[test]
fn adopt_undo_catalog_failure_stops_writes_and_recovers_forward_on_restart() {
    let harness = Harness::new();
    let claude = harness.claude_skills();
    let codex = harness.codex_skills();
    let shared = harness.shared_skills();
    write_skill(&shared, "undo-fault", "# Undo fault\n");
    let recovery_gate = Arc::new(RecoveryGate::ready());
    let adopt = AdoptService::new(
        Arc::new(FailFirstAdoptRemovalStore::new(harness.runtime.clone())),
        harness.filesystem.clone(),
        Arc::new(SystemClock::new()),
        harness.library_root.clone(),
        harness.home.path().to_path_buf(),
    )
    .with_recovery_gate(recovery_gate.clone());
    let candidates = adopt.scan().expect("scan").candidates;
    let plan = adopt
        .plan(&[select(&candidates, "undo-fault")])
        .expect("plan Adopt");
    let result = adopt.apply(&plan.plan_token).expect("apply Adopt");
    let library_entity = harness.entity("undo-fault");

    let error = adopt
        .undo(&result.operation_id)
        .expect_err("a failure after Undo starts needs durable recovery");

    assert!(matches!(error, AdoptError::RecoveryRequired(_)), "{error}");
    assert!(!recovery_gate.writes_are_ready());
    assert!(
        harness
            .library_root
            .join("operations")
            .join(&result.operation_id)
            .join("adopt-journal.json")
            .is_file(),
        "the journal must remain until startup reconciles the catalog and filesystem"
    );
    assert!(library_entity.join("SKILL.md").is_file());
    assert!(
        !shared.join("undo-fault").exists(),
        "catalog deletion happens before restoring the shared source"
    );
    assert!(
        std::fs::symlink_metadata(claude.join("undo-fault")).is_err(),
        "the injected failure happens after owned Activations are removed"
    );
    assert!(
        std::fs::symlink_metadata(codex.join("undo-fault")).is_err(),
        "all owned Activations are removed before the catalog mutation"
    );
    assert!(
        harness
            .runtime
            .load_file_install("undo-fault")
            .expect("load adopted source")
            .is_some(),
        "the injected catalog failure leaves the durable row present"
    );

    let restart_gate = Arc::new(RecoveryGate::blocked());
    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .with_recovery_gate(restart_gate.clone())
        .startup_check()
        .expect("startup uses the catalog row to recover the interrupted Undo forward");

    assert!(restart_gate.writes_are_ready());
    assert_eq!(
        std::fs::read_link(claude.join("undo-fault")).expect("restored Claude Activation"),
        library_entity
    );
    assert_eq!(
        std::fs::read_link(codex.join("undo-fault")).expect("restored Codex Activation"),
        library_entity
    );
    assert!(
        !harness
            .library_root
            .join("operations")
            .join(&result.operation_id)
            .exists(),
        "startup archives the reconciled Undo journal"
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
    let ghost_hash = filesystem.tree_hash(&ghost).expect("hash ghost entity");

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
                source_fingerprint: None,
                staged_fingerprint: skill_man_lib::seams::filesystem::DirectoryFingerprint {
                    canonical_path: PathBuf::new(),
                    device: 0,
                    inode: 0,
                },
                final_entity_path: ghost.clone(),
                recorded_content_hash: ghost_hash,
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
                source_fingerprint: None,
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

fn assert_startup_recovery_restores_a_legacy_staged_source(
    journal_phase: skill_man_lib::seams::filesystem::AdoptJournalPhase,
) {
    let harness = Harness::new();
    let shared = harness.shared_skills();
    let source = write_skill(&shared, "legacy-preview", "# Legacy Preview\n");
    let operation_id = "adopt-legacy-preview";
    let staging_operation_root = harness.library_root.join("staging").join(operation_id);
    let staged_root = staging_operation_root.join("legacy-preview");
    let staged_fingerprint = harness
        .filesystem
        .stage_external_directory(&source, &staged_root)
        .expect("mimic the legacy Preview staging move");
    let staging_fingerprint = harness
        .filesystem
        .directory_fingerprint(&staging_operation_root)
        .expect("fingerprint legacy staging root");
    let recorded_content_hash = harness
        .filesystem
        .tree_hash(&staged_root)
        .expect("hash staged Skill");
    let journal = skill_man_lib::seams::filesystem::AdoptJournal {
        version: 1,
        operation_id: operation_id.into(),
        phase: journal_phase,
        staging_operation_root: staging_operation_root.clone(),
        staging_fingerprint,
        items: vec![skill_man_lib::seams::filesystem::AdoptJournalItem {
            skill_id: "legacy-preview-skill".into(),
            directory_name: "legacy-preview".into(),
            kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Migrate,
            staged_root,
            source_fingerprint: None,
            staged_fingerprint,
            final_entity_path: harness.entity("legacy-preview"),
            recorded_content_hash,
            original_path: source.clone(),
            original_filename: "legacy-preview".into(),
            appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
                entry_path: source.clone(),
                kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::RealDirectory,
            }],
            activations: Vec::new(),
            phase: skill_man_lib::seams::filesystem::AdoptItemPhase::Staged,
            installed_fingerprint: None,
        }],
    };
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &journal)
        .expect("write legacy Adopt journal");
    let journal_path = harness
        .library_root
        .join("operations")
        .join(operation_id)
        .join("adopt-journal.json");
    let mut legacy_json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&journal_path).expect("read serialized legacy Adopt journal"),
    )
    .expect("parse serialized legacy Adopt journal");
    legacy_json["items"][0]
        .as_object_mut()
        .expect("legacy journal item")
        .remove("source_fingerprint");
    assert!(
        legacy_json["items"][0].get("source_fingerprint").is_none(),
        "the compatibility fixture must omit the field like a real v1 journal"
    );
    std::fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&legacy_json).expect("serialize legacy journal without field"),
    )
    .expect("write legacy journal without source_fingerprint");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect("startup recovery");

    assert!(
        source.join("SKILL.md").is_file(),
        "startup must restore the Preview-moved Skill before orphan cleanup"
    );
    assert!(!staging_operation_root.exists());
}

#[test]
fn startup_recovery_restores_a_legacy_preview_before_cleaning_adopt_staging() {
    assert_startup_recovery_restores_a_legacy_staged_source(
        skill_man_lib::seams::filesystem::AdoptJournalPhase::Planned,
    );
}

#[test]
fn startup_recovery_restores_a_legacy_applying_staged_source() {
    assert_startup_recovery_restores_a_legacy_staged_source(
        skill_man_lib::seams::filesystem::AdoptJournalPhase::Applying,
    );
}

#[test]
fn startup_recovery_prefers_an_isolated_cross_volume_source_over_the_older_copy() {
    let harness = Harness::new();
    let operation_id = "adopt-isolated-crash";
    let directory_name = "isolated-crash";
    let source = write_skill(
        &harness.shared_skills(),
        directory_name,
        "# Before cross-volume copy\n",
    );
    let source_fingerprint = harness
        .filesystem
        .directory_fingerprint(&source)
        .expect("fingerprint source before copy");
    let original_hash = harness
        .filesystem
        .tree_hash(&source)
        .expect("hash source before copy");
    let staging_operation_root = harness.library_root.join("staging").join(operation_id);
    let staged_root = staging_operation_root.join(directory_name);
    std::fs::create_dir_all(&staged_root).expect("create copied staging tree");
    std::fs::copy(source.join("SKILL.md"), staged_root.join("SKILL.md"))
        .expect("copy original Skill into staging");
    let staging_fingerprint = harness
        .filesystem
        .directory_fingerprint(&staging_operation_root)
        .expect("fingerprint staging operation");

    std::fs::write(source.join("late.txt"), "must survive recovery\n")
        .expect("write content after the cross-volume copy");
    let isolated = source.parent().expect("source parent").join(format!(
        ".{operation_id}-source-{}",
        source_fingerprint.inode
    ));
    std::fs::rename(&source, &isolated).expect("simulate crash after source isolation");

    let journal = skill_man_lib::seams::filesystem::AdoptJournal {
        version: 2,
        operation_id: operation_id.into(),
        phase: skill_man_lib::seams::filesystem::AdoptJournalPhase::Applying,
        staging_operation_root: staging_operation_root.clone(),
        staging_fingerprint,
        items: vec![skill_man_lib::seams::filesystem::AdoptJournalItem {
            skill_id: "isolated-crash-skill".into(),
            directory_name: directory_name.into(),
            kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Migrate,
            staged_root,
            // A Planned v2 item still records the external source identity.
            source_fingerprint: Some(source_fingerprint.clone()),
            staged_fingerprint: source_fingerprint,
            final_entity_path: harness.entity(directory_name),
            recorded_content_hash: original_hash,
            original_path: source.clone(),
            original_filename: directory_name.into(),
            appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
                entry_path: source.clone(),
                kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::RealDirectory,
            }],
            activations: Vec::new(),
            phase: skill_man_lib::seams::filesystem::AdoptItemPhase::Planned,
            installed_fingerprint: None,
        }],
    };
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &journal)
        .expect("write interrupted isolation journal");

    let recovery_gate = Arc::new(RecoveryGate::blocked());
    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .with_recovery_gate(recovery_gate.clone())
        .startup_check()
        .expect("recover isolated source");

    assert!(recovery_gate.writes_are_ready());
    assert_eq!(
        std::fs::read_to_string(source.join("late.txt")).expect("late content was restored"),
        "must survive recovery\n"
    );
    assert!(!isolated.exists());
    assert!(!staging_operation_root.exists());
    assert!(
        !harness
            .library_root
            .join("operations")
            .join(operation_id)
            .exists()
    );
}

#[test]
fn startup_recovery_uses_durable_staging_after_isolated_source_cleanup_started() {
    let harness = Harness::new();
    let operation_id = "adopt-isolated-cleanup-crash";
    let directory_name = "isolated-cleanup-crash";
    let source = write_skill(
        &harness.shared_skills(),
        directory_name,
        "# Complete staged source\n",
    );
    std::fs::write(source.join("helper.txt"), "complete helper\n")
        .expect("write second source file");
    let source_fingerprint = harness
        .filesystem
        .directory_fingerprint(&source)
        .expect("fingerprint source before isolation");
    let recorded_content_hash = harness
        .filesystem
        .tree_hash(&source)
        .expect("hash complete source");
    let staging_operation_root = harness.library_root.join("staging").join(operation_id);
    let staged_root = staging_operation_root.join(directory_name);
    std::fs::create_dir_all(&staged_root).expect("create durable staging copy");
    std::fs::copy(source.join("SKILL.md"), staged_root.join("SKILL.md"))
        .expect("copy staged Skill document");
    std::fs::copy(source.join("helper.txt"), staged_root.join("helper.txt"))
        .expect("copy staged helper");
    let staging_fingerprint = harness
        .filesystem
        .directory_fingerprint(&staging_operation_root)
        .expect("fingerprint staging operation");
    let staged_fingerprint = harness
        .filesystem
        .directory_fingerprint(&staged_root)
        .expect("fingerprint durable staging copy");

    let isolated = source.parent().expect("source parent").join(format!(
        ".{operation_id}-source-{}",
        source_fingerprint.inode
    ));
    std::fs::rename(&source, &isolated).expect("isolate copied source");
    std::fs::remove_file(isolated.join("helper.txt"))
        .expect("simulate interrupted recursive cleanup");

    let journal = skill_man_lib::seams::filesystem::AdoptJournal {
        version: 2,
        operation_id: operation_id.into(),
        phase: skill_man_lib::seams::filesystem::AdoptJournalPhase::Applying,
        staging_operation_root: staging_operation_root.clone(),
        staging_fingerprint,
        items: vec![skill_man_lib::seams::filesystem::AdoptJournalItem {
            skill_id: "isolated-cleanup-crash-skill".into(),
            directory_name: directory_name.into(),
            kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Migrate,
            staged_root,
            source_fingerprint: Some(source_fingerprint),
            staged_fingerprint,
            final_entity_path: harness.entity(directory_name),
            recorded_content_hash,
            original_path: source.clone(),
            original_filename: directory_name.into(),
            appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
                entry_path: source.clone(),
                kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::RealDirectory,
            }],
            activations: Vec::new(),
            phase: skill_man_lib::seams::filesystem::AdoptItemPhase::Staged,
            installed_fingerprint: None,
        }],
    };
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &journal)
        .expect("write interrupted cleanup journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect("recover from isolated cleanup crash");

    assert_eq!(
        std::fs::read_to_string(source.join("SKILL.md")).expect("Skill document restored"),
        "# Complete staged source\n"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("helper.txt")).expect("helper restored"),
        "complete helper\n"
    );
    assert!(!isolated.exists());
    assert!(!staging_operation_root.exists());
}

#[test]
fn startup_recovery_retains_orphaned_adopt_staging_without_a_journal() {
    let harness = Harness::new();
    let orphaned = harness
        .library_root
        .join("staging/adopt-pre-journal-crash/ask-matt");
    std::fs::create_dir_all(&orphaned).expect("create orphaned Adopt staging");
    std::fs::write(orphaned.join("SKILL.md"), "# Ask Matt\n").expect("write the only staged copy");

    let error = MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect_err("unknown Adopt staging must require manual recovery");

    assert!(error.to_string().contains("orphaned Adopt staging"));
    assert!(
        orphaned.join("SKILL.md").is_file(),
        "startup must retain the only possible copy"
    );
}

#[test]
fn file_import_cleanup_never_deletes_staging_owned_by_an_adopt_journal() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "file-import-adopt-collision",
        "cross-family-adopt",
    );
    fixture.journal.version = 1;
    fixture.journal.phase = skill_man_lib::seams::filesystem::AdoptJournalPhase::Planned;
    let operation_root = harness
        .library_root
        .join("operations/file-import-adopt-collision");
    std::fs::create_dir_all(&operation_root).expect("create cross-family operation fixture");
    std::fs::write(
        operation_root.join("adopt-journal.json"),
        serde_json::to_vec_pretty(&fixture.journal).expect("serialize Adopt journal fixture"),
    )
    .expect("write cross-family Adopt journal fixture");

    let _ = MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect_err("the malformed family id must fail closed");

    assert!(
        fixture.staged_root.join("SKILL.md").is_file(),
        "File Import recovery must retain staging referenced by an Adopt journal"
    );
    assert!(!fixture.source.exists());
}

#[test]
fn startup_recovery_restores_an_adopt_entity_left_at_the_install_temporary_path() {
    let harness = Harness::new();
    let fixture = staged_adopt_fixture(&harness, "adopt-install-temporary", "install-temporary");
    let skills_root = harness.library_root.join("skills");
    std::fs::create_dir_all(&skills_root).expect("create Library skills root");
    let temporary_path = skills_root.join(".install-temporary.new-adopt-install-temporary");
    std::fs::rename(&fixture.staged_root, &temporary_path).expect("mimic the first install rename");
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write interrupted Adopt journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect("startup restores the temporary entity");

    assert!(fixture.source.join("SKILL.md").is_file());
    assert!(!temporary_path.exists());
    assert!(!fixture.staging_operation_root.exists());
}

#[test]
fn startup_recovery_retains_a_changed_staged_adopt_entity() {
    let harness = Harness::new();
    let fixture = staged_adopt_fixture(&harness, "adopt-changed-staging", "changed-staging");
    std::fs::write(
        fixture.staged_root.join("SKILL.md"),
        "# Externally changed\n",
    )
    .expect("change staged content after the journal snapshot");
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write interrupted Adopt journal");

    let _ = MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect_err("changed staged content requires manual recovery");

    assert!(!fixture.source.exists());
    assert_eq!(
        std::fs::read_to_string(fixture.staged_root.join("SKILL.md"))
            .expect("changed staged copy is retained"),
        "# Externally changed\n"
    );
    assert!(
        harness
            .library_root
            .join("operations/adopt-changed-staging/adopt-journal.json")
            .is_file()
    );
}

#[test]
fn startup_recovery_retains_a_committed_adopt_when_the_final_entity_is_missing() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "adopt-committed-missing-final",
        "committed-missing-final",
    );
    let activation = harness.claude_skills().join("committed-missing-final");
    let item = &mut fixture.journal.items[0];
    item.phase = skill_man_lib::seams::filesystem::AdoptItemPhase::CatalogCommitted;
    item.installed_fingerprint = Some(item.staged_fingerprint.clone());
    item.activations = vec![skill_man_lib::seams::filesystem::AdoptActivationStep {
        agent_id: "claude-code".into(),
        entry_path: activation.clone(),
        target_path: item.final_entity_path.clone(),
    }];
    insert_committed_migrate_row(&harness, item, &fixture.source);
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write interrupted committed Adopt journal");

    let error = MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect_err("a committed Adopt without its final entity requires recovery");

    assert!(error.to_string().contains("final entity"), "{error}");
    assert!(
        fixture.staged_root.join("SKILL.md").is_file(),
        "the staged tree may be the only copy and must be retained"
    );
    assert!(!fixture.source.exists());
    assert!(!fixture.journal.items[0].final_entity_path.exists());
    assert!(!activation.exists(), "no dangling Activation is created");
    assert!(
        harness
            .library_root
            .join("operations/adopt-committed-missing-final/adopt-journal.json")
            .is_file(),
        "the journal remains available for recovery"
    );
}

#[test]
fn startup_recovery_retains_a_committed_adopt_at_the_install_temporary_path() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "adopt-committed-install-temporary",
        "committed-install-temporary",
    );
    let skills_root = harness.library_root.join("skills");
    std::fs::create_dir_all(&skills_root).expect("create Library skills root");
    let temporary_path =
        skills_root.join(".committed-install-temporary.new-adopt-committed-install-temporary");
    std::fs::rename(&fixture.staged_root, &temporary_path).expect("mimic the first install rename");
    let item = &mut fixture.journal.items[0];
    item.phase = skill_man_lib::seams::filesystem::AdoptItemPhase::CatalogCommitted;
    item.installed_fingerprint = Some(item.staged_fingerprint.clone());
    insert_committed_migrate_row(&harness, item, &fixture.source);
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write committed temporary-path journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect_err("a committed Adopt without its final entity requires recovery");

    assert!(
        temporary_path.join("SKILL.md").is_file(),
        "the temporary tree may be the only copy and must be retained"
    );
    assert!(!fixture.source.exists());
    assert!(!fixture.journal.items[0].final_entity_path.exists());
    assert!(
        harness
            .library_root
            .join("operations/adopt-committed-install-temporary/adopt-journal.json")
            .is_file()
    );
}

#[test]
fn startup_recovery_retains_a_committed_adopt_with_a_changed_final_entity() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "adopt-committed-changed-final",
        "committed-changed-final",
    );
    let staged_snapshot = harness
        .filesystem
        .staged_tree_snapshot(&fixture.staged_root)
        .expect("snapshot staged fixture");
    let installed_fingerprint = harness
        .filesystem
        .install_staged_skill(
            &fixture.staged_root,
            &fixture.journal.items[0].final_entity_path,
            &harness.library_root,
            &fixture.journal.operation_id,
            &staged_snapshot,
        )
        .expect("mimic the installed Adopt entity");
    let item = &mut fixture.journal.items[0];
    item.phase = skill_man_lib::seams::filesystem::AdoptItemPhase::CatalogCommitted;
    item.installed_fingerprint = Some(installed_fingerprint);
    insert_committed_migrate_row(&harness, item, &fixture.source);
    std::fs::write(
        item.final_entity_path.join("SKILL.md"),
        "# Externally changed final\n",
    )
    .expect("change the committed final entity");
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write changed-final journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect_err("a changed committed final entity requires recovery");

    assert_eq!(
        std::fs::read_to_string(fixture.journal.items[0].final_entity_path.join("SKILL.md"))
            .expect("changed final entity is retained"),
        "# Externally changed final\n"
    );
    assert!(
        harness
            .library_root
            .join("operations/adopt-committed-changed-final/adopt-journal.json")
            .is_file()
    );
}

#[test]
fn startup_recovery_retains_a_committed_adopt_with_a_replaced_final_entity() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "adopt-committed-replaced-final",
        "committed-replaced-final",
    );
    let staged_snapshot = harness
        .filesystem
        .staged_tree_snapshot(&fixture.staged_root)
        .expect("snapshot staged fixture");
    let installed_fingerprint = harness
        .filesystem
        .install_staged_skill(
            &fixture.staged_root,
            &fixture.journal.items[0].final_entity_path,
            &harness.library_root,
            &fixture.journal.operation_id,
            &staged_snapshot,
        )
        .expect("mimic the installed Adopt entity");
    let item = &mut fixture.journal.items[0];
    item.phase = skill_man_lib::seams::filesystem::AdoptItemPhase::CatalogCommitted;
    item.installed_fingerprint = Some(installed_fingerprint);
    insert_committed_migrate_row(&harness, item, &fixture.source);
    let preserved_original = harness
        .home
        .path()
        .join("Preserved/committed-replaced-final");
    std::fs::create_dir_all(preserved_original.parent().expect("preserved parent"))
        .expect("create preserved parent");
    std::fs::rename(&item.final_entity_path, &preserved_original)
        .expect("move the original final entity aside");
    write_skill(
        item.final_entity_path
            .parent()
            .expect("Library skills root"),
        "committed-replaced-final",
        "# committed-replaced-final\n",
    );
    assert_eq!(
        harness
            .filesystem
            .tree_hash(&item.final_entity_path)
            .expect("hash replacement final"),
        item.recorded_content_hash,
        "the replacement deliberately has the same content but a different identity"
    );
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write replaced-final journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect_err("a replaced committed final entity requires recovery");

    assert!(preserved_original.join("SKILL.md").is_file());
    assert!(
        fixture.journal.items[0]
            .final_entity_path
            .join("SKILL.md")
            .is_file()
    );
    assert!(
        harness
            .library_root
            .join("operations/adopt-committed-replaced-final/adopt-journal.json")
            .is_file()
    );
}

#[test]
fn startup_recovery_reverses_a_done_cursor_when_the_catalog_row_is_absent() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "adopt-done-without-catalog",
        "done-without-catalog",
    );
    let staged_snapshot = harness
        .filesystem
        .staged_tree_snapshot(&fixture.staged_root)
        .expect("snapshot staged fixture");
    let installed_fingerprint = harness
        .filesystem
        .install_staged_skill(
            &fixture.staged_root,
            &fixture.journal.items[0].final_entity_path,
            &harness.library_root,
            &fixture.journal.operation_id,
            &staged_snapshot,
        )
        .expect("mimic an installed Adopt item");
    fixture.journal.items[0].installed_fingerprint = Some(installed_fingerprint);
    fixture.journal.items[0].phase = skill_man_lib::seams::filesystem::AdoptItemPhase::Done;
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write ambiguous Done journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect("catalog absence makes recovery reverse the item");

    assert!(fixture.source.join("SKILL.md").is_file());
    assert!(!fixture.journal.items[0].final_entity_path.exists());
}

#[test]
fn startup_recovery_restores_original_symlink_appearances_when_the_catalog_is_absent() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "adopt-restore-symlink-appearances",
        "migrated-appearance",
    );
    let claude = harness.claude_skills();
    let migrated_appearance = claude.join("migrated-appearance");
    let migrated_target = fixture.source.clone();
    fixture.journal.items[0].appearances.push(
        skill_man_lib::seams::filesystem::AdoptAppearanceStep {
            entry_path: migrated_appearance.clone(),
            kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::Symlink {
                original_target: migrated_target.clone(),
            },
        },
    );
    let migrated_final = fixture.journal.items[0].final_entity_path.clone();
    fixture.journal.items[0].activations.push(
        skill_man_lib::seams::filesystem::AdoptActivationStep {
            agent_id: "claude-code".into(),
            entry_path: migrated_appearance.clone(),
            target_path: migrated_final,
        },
    );
    fixture.journal.items[0].phase = skill_man_lib::seams::filesystem::AdoptItemPhase::Done;

    let external = write_skill(
        &harness.home.path().join("Projects"),
        "linked-appearance",
        "# Linked appearance\n",
    );
    let linked_appearance = claude.join("linked-appearance");
    let link_fingerprint = harness
        .filesystem
        .directory_fingerprint(&external)
        .expect("fingerprint external Link entity");
    fixture
        .journal
        .items
        .push(skill_man_lib::seams::filesystem::AdoptJournalItem {
            skill_id: "linked-appearance-skill".into(),
            directory_name: "linked-appearance".into(),
            kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Link,
            staged_root: PathBuf::new(),
            source_fingerprint: None,
            staged_fingerprint: link_fingerprint,
            final_entity_path: external.clone(),
            recorded_content_hash: String::new(),
            original_path: external.clone(),
            original_filename: "linked-appearance".into(),
            appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
                entry_path: linked_appearance.clone(),
                kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::Symlink {
                    original_target: external.clone(),
                },
            }],
            activations: vec![skill_man_lib::seams::filesystem::AdoptActivationStep {
                agent_id: "claude-code".into(),
                entry_path: linked_appearance.clone(),
                target_path: external.clone(),
            }],
            phase: skill_man_lib::seams::filesystem::AdoptItemPhase::Done,
            installed_fingerprint: None,
        });
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write interrupted reverse Adopt journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect("catalog absence restores every original appearance");

    assert!(fixture.source.join("SKILL.md").is_file());
    assert_eq!(
        std::fs::read_link(&migrated_appearance).expect("restored migrated appearance"),
        migrated_target
    );
    assert_eq!(
        std::fs::read_link(&linked_appearance).expect("restored Link appearance"),
        external
    );
}

#[test]
fn startup_recovery_accepts_an_appearance_already_replaced_by_its_planned_activation() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "adopt-partial-appearance-forward",
        "partial-appearance-forward",
    );
    let staged_snapshot = harness
        .filesystem
        .staged_tree_snapshot(&fixture.staged_root)
        .expect("snapshot staged fixture");
    let installed_fingerprint = harness
        .filesystem
        .install_staged_skill(
            &fixture.staged_root,
            &fixture.journal.items[0].final_entity_path,
            &harness.library_root,
            &fixture.journal.operation_id,
            &staged_snapshot,
        )
        .expect("mimic the installed Adopt entity");
    let activation = harness.claude_skills().join("partial-appearance-forward");
    let item = &mut fixture.journal.items[0];
    item.installed_fingerprint = Some(installed_fingerprint);
    item.phase = skill_man_lib::seams::filesystem::AdoptItemPhase::CatalogCommitted;
    item.appearances
        .push(skill_man_lib::seams::filesystem::AdoptAppearanceStep {
            entry_path: activation.clone(),
            kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::Symlink {
                original_target: fixture.source.clone(),
            },
        });
    item.activations = vec![skill_man_lib::seams::filesystem::AdoptActivationStep {
        agent_id: "claude-code".into(),
        entry_path: activation.clone(),
        target_path: item.final_entity_path.clone(),
    }];
    std::os::unix::fs::symlink(&item.final_entity_path, &activation)
        .expect("mimic an Activation created before the crash");
    insert_committed_migrate_row(&harness, item, &fixture.source);
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write interrupted appearance journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect("the already-created planned Activation is replay-safe");

    assert_eq!(
        std::fs::read_link(&activation).expect("forwarded Activation remains"),
        fixture.journal.items[0].final_entity_path
    );
    assert!(
        fixture.journal.items[0]
            .final_entity_path
            .join("SKILL.md")
            .is_file()
    );
    assert!(
        !harness
            .library_root
            .join("operations/adopt-partial-appearance-forward")
            .exists(),
        "the converged journal is archived"
    );
}

#[test]
fn startup_recovery_converges_each_item_in_a_committed_adopt_journal() {
    let harness = Harness::new();
    let mut fixture = staged_adopt_fixture(
        &harness,
        "adopt-committed-item-recovery",
        "committed-reverse",
    );
    fixture.journal.phase = skill_man_lib::seams::filesystem::AdoptJournalPhase::Committed;
    fixture.journal.items[0].phase = skill_man_lib::seams::filesystem::AdoptItemPhase::Done;

    let external = write_skill(
        &harness.home.path().join("Projects"),
        "committed-forward",
        "# Committed forward\n",
    );
    let activation = harness.claude_skills().join("committed-forward");
    let link_fingerprint = harness
        .filesystem
        .directory_fingerprint(&external)
        .expect("fingerprint committed Link entity");
    let link_item = skill_man_lib::seams::filesystem::AdoptJournalItem {
        skill_id: "committed-forward-skill".into(),
        directory_name: "committed-forward".into(),
        kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Link,
        staged_root: PathBuf::new(),
        source_fingerprint: None,
        staged_fingerprint: link_fingerprint,
        final_entity_path: external.clone(),
        recorded_content_hash: String::new(),
        original_path: external.clone(),
        original_filename: "committed-forward".into(),
        appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
            entry_path: activation.clone(),
            kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::Symlink {
                original_target: external.clone(),
            },
        }],
        activations: vec![skill_man_lib::seams::filesystem::AdoptActivationStep {
            agent_id: "claude-code".into(),
            entry_path: activation.clone(),
            target_path: external.clone(),
        }],
        phase: skill_man_lib::seams::filesystem::AdoptItemPhase::Done,
        installed_fingerprint: None,
    };
    AdoptStore::insert_adopted(
        harness.runtime.as_ref(),
        AdoptedSkillRecord {
            skill_id: SkillId(link_item.skill_id.clone()),
            directory_name: link_item.directory_name.clone(),
            identity_key: link_item.directory_name.clone(),
            display_name: link_item.directory_name.clone(),
            description: String::new(),
            library_entry_path: None,
            final_entity_path: external.clone(),
            recorded_content_hash: None,
            original_path: None,
            original_filename: link_item.original_filename.clone(),
            activations: vec![skill_man_lib::seams::adopt_store::AdoptedActivation {
                agent_id: AgentId("claude-code".into()),
                expected_entry_path: activation.clone(),
                expected_target_path: external.clone(),
            }],
        },
    )
    .expect("insert committed Link row");
    fixture.journal.items.push(link_item);
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &fixture.journal)
        .expect("write committed Adopt recovery journal");

    MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect("Committed recovery converges from the catalog oracle");

    assert!(
        fixture.source.join("SKILL.md").is_file(),
        "the catalog-absent item is reversed before staging cleanup"
    );
    assert_eq!(
        std::fs::read_link(&activation).expect("catalog-present item is forwarded"),
        external
    );
    assert!(
        !harness
            .library_root
            .join("operations/adopt-committed-item-recovery")
            .exists()
    );
}

#[test]
fn startup_recovery_retains_an_unknown_adopt_journal_version() {
    let harness = Harness::new();
    let shared = harness.shared_skills();
    let source = write_skill(&shared, "future-version", "# Future Version\n");
    let operation_id = "adopt-future-version";
    let staging_operation_root = harness.library_root.join("staging").join(operation_id);
    let staged_root = staging_operation_root.join("future-version");
    let staged_fingerprint = harness
        .filesystem
        .stage_external_directory(&source, &staged_root)
        .expect("stage future-version fixture");
    let staging_fingerprint = harness
        .filesystem
        .directory_fingerprint(&staging_operation_root)
        .expect("fingerprint staging root");
    let recorded_content_hash = harness
        .filesystem
        .tree_hash(&staged_root)
        .expect("hash staged Skill");
    let journal = skill_man_lib::seams::filesystem::AdoptJournal {
        version: 99,
        operation_id: operation_id.into(),
        phase: skill_man_lib::seams::filesystem::AdoptJournalPhase::Planned,
        staging_operation_root: staging_operation_root.clone(),
        staging_fingerprint,
        items: vec![skill_man_lib::seams::filesystem::AdoptJournalItem {
            skill_id: "future-version-skill".into(),
            directory_name: "future-version".into(),
            kind: skill_man_lib::seams::filesystem::AdoptJournalKind::Migrate,
            staged_root: staged_root.clone(),
            source_fingerprint: None,
            staged_fingerprint,
            final_entity_path: harness.entity("future-version"),
            recorded_content_hash,
            original_path: source.clone(),
            original_filename: "future-version".into(),
            appearances: vec![skill_man_lib::seams::filesystem::AdoptAppearanceStep {
                entry_path: source.clone(),
                kind: skill_man_lib::seams::filesystem::AdoptAppearanceKind::RealDirectory,
            }],
            activations: Vec::new(),
            phase: skill_man_lib::seams::filesystem::AdoptItemPhase::Staged,
            installed_fingerprint: None,
        }],
    };
    harness
        .filesystem
        .write_adopt_journal(&harness.library_root, &journal)
        .expect("write future Adopt journal");

    let error = MaintenanceService::new(harness.runtime.clone(), harness.filesystem.clone())
        .with_library_root(harness.library_root.clone())
        .startup_check()
        .expect_err("unknown journal versions must fail closed");

    assert!(
        error
            .to_string()
            .contains("unsupported Adopt journal version")
    );
    assert!(staged_root.join("SKILL.md").is_file());
    assert!(!source.exists());
}
