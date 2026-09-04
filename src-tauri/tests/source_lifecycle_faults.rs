//! Service-level Source lifecycle fault tests (ticket #93, spec §8.3–§8.4,
//! ADR-0018). Every commit point of Restore Current Source Release, Create
//! Local Source Copy and whole-source Remove gets an injected fault or a
//! hand-frozen crash journal, and recovery must roll back before the commit
//! point and roll forward after it — never silently overwrite observed
//! bytes. Store-level commit/undo/presence facts live in
//! `source_update_lifecycle.rs`; transition recovery lives in
//! `source_transition.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use rusqlite::Connection;
use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::domain::{Health, SkillId};
use skill_man_lib::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreviewOutcome, SourceGroupPreviewService,
    SourceTrackingOverride,
};
use skill_man_lib::core::source_lifecycle::{SourceLifecycleError, SourceLifecycleService};
use skill_man_lib::core::source_transition::{
    ConfirmSourceTransitionRequest, SourceTransitionError, SourceTransitionService,
};
use skill_man_lib::core::source_update::{
    SourceUpdateError, SourceUpdateMemberState, SourceUpdateService,
};
use skill_man_lib::core::write_gate::WriteGate;
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::filesystem::{
    FileSystem, LocalCopyJournal, LocalCopyPhase, RemoveSourceActivationJournal,
    RemoveSourceJournal, RemoveSourceMemberJournal, RemoveSourcePhase, RestoreSourceJournal,
    RestoreSourceMember, RestoreSourcePhase, SourceLifecycleJournal,
};
use skill_man_lib::seams::source::{GitFetchReport, GitSource, GitTreeEntry, SourceError};
use skill_man_lib::seams::source_update_store::{
    LocalSourceCopyRecord, SourceMemberPresence, SourceUpdateCurrentSource, SourceUpdateRecord,
    SourceUpdateStore, SourceUpdateStoreError,
};

const FIXTURE_URL: &str = "https://example.com/acme/source";
const ALPHA_SKILL_MD: &str = "---\nname: Source Root\ndescription: Root member\n---\n# Root\n";

// ---- Harness ------------------------------------------------------------

fn git(repository: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_file(root: &Path, path: &str, contents: &str) {
    let target = root.join(path);
    std::fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
    std::fs::write(target, contents).expect("write fixture");
}

fn fixture_repository(root: &Path) -> PathBuf {
    let repository = root.join("source-repository");
    std::fs::create_dir_all(&repository).expect("repository");
    write_file(&repository, "skills/alpha/SKILL.md", ALPHA_SKILL_MD);
    write_file(
        &repository,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: Nested member\n---\n# Beta\n",
    );
    git(&repository, &["init", "-q", "-b", "main"]);
    git(
        &repository,
        &["config", "user.email", "fixture@example.com"],
    );
    git(&repository, &["config", "user.name", "Fixture"]);
    git(&repository, &["add", "-A"]);
    git(
        &repository,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "release",
        ],
    );
    repository
}

struct FixtureGitSource {
    inner: SystemGitSource,
    fixture_url: String,
}

impl FixtureGitSource {
    fn new(repository: &Path) -> Self {
        Self {
            inner: SystemGitSource::new(),
            fixture_url: format!("file://{}", repository.display()),
        }
    }
}

impl GitSource for FixtureGitSource {
    fn fetch_mirror(&self, url: &str, mirror_dir: &Path) -> Result<GitFetchReport, SourceError> {
        let resolved = if url == FIXTURE_URL {
            self.fixture_url.as_str()
        } else {
            url
        };
        self.inner.fetch_mirror(resolved, mirror_dir)
    }

    fn resolve_commit(&self, mirror_dir: &Path, rev: &str) -> Result<Option<String>, SourceError> {
        self.inner.resolve_commit(mirror_dir, rev)
    }

    fn list_tree(&self, mirror_dir: &Path, commit: &str) -> Result<Vec<GitTreeEntry>, SourceError> {
        self.inner.list_tree(mirror_dir, commit)
    }

    fn read_blob(
        &self,
        mirror_dir: &Path,
        commit: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, SourceError> {
        self.inner.read_blob(mirror_dir, commit, path, max_bytes)
    }

    fn tree_summary(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
    ) -> Result<String, SourceError> {
        self.inner.tree_summary(mirror_dir, commit, skill_path)
    }

    fn stage_skill(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
        destination: &Path,
    ) -> Result<(), SourceError> {
        self.inner
            .stage_skill(mirror_dir, commit, skill_path, destination)
    }

    fn list_tags(
        &self,
        mirror_dir: &Path,
    ) -> Result<Vec<skill_man_lib::seams::source::GitTagFact>, SourceError> {
        self.inner.list_tags(mirror_dir)
    }

    fn is_ancestor(
        &self,
        mirror_dir: &Path,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, SourceError> {
        self.inner.is_ancestor(mirror_dir, ancestor, descendant)
    }
}

#[derive(Default)]
struct FixtureClock;

impl Clock for FixtureClock {
    fn monotonic_millis(&self) -> u128 {
        1
    }

    fn unix_epoch_nanos(&self) -> u128 {
        1_725_000_000_000_000_000
    }
}

struct Fixture {
    _workspace: tempfile::TempDir,
    repository: PathBuf,
    library: PathBuf,
    home: PathBuf,
    export_root: PathBuf,
    filesystem: Arc<MacOsFileSystem>,
    catalog: Arc<SqliteCatalogStore>,
    source: Arc<dyn GitSource>,
    locks: Arc<SystemInstallerLockStore>,
    transition: Arc<SourceTransitionService>,
    preview: Arc<SourceGroupPreviewService>,
    lifecycle: Arc<SourceLifecycleService>,
    update: Arc<SourceUpdateService>,
    lock_path: PathBuf,
    write_gate: Arc<WriteGate>,
}

/// A managed v9 source with two members (alpha, beta) plus one desired
/// Activation for alpha, built through the real production transition.
fn fixture() -> Fixture {
    let workspace = tempfile::tempdir().expect("workspace");
    let repository = fixture_repository(workspace.path());
    let home = workspace.path().join("home");
    let library = workspace.path().join("library");
    let export_root = workspace.path().join("export");
    std::fs::create_dir_all(&library).expect("Library");
    std::fs::create_dir_all(&export_root).expect("export root");
    let source_root = home.join(".agents/skills");
    write_file(&source_root.join("alpha"), "SKILL.md", ALPHA_SKILL_MD);
    write_file(
        &source_root.join("beta"),
        "SKILL.md",
        "---\nname: Source Beta\ndescription: Nested member\n---\n# Beta\n",
    );
    let lock_path = home.join(".agents/.skill-lock.json");
    std::fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("lock parent");
    std::fs::write(
        &lock_path,
        r#"{
  "version": 3,
  "skills": {
    "alpha": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/alpha",
      "skillFolderHash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    },
    "beta": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/beta",
      "skillFolderHash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  }
}"#,
    )
    .expect("write lock");

    let source: Arc<dyn GitSource> = Arc::new(FixtureGitSource::new(&repository));
    let locks = Arc::new(SystemInstallerLockStore::new(home.clone()));
    let preview = Arc::new(SourceGroupPreviewService::new(
        source.clone(),
        locks.clone(),
    ));
    let filesystem = Arc::new(MacOsFileSystem::new(home.clone()));
    let catalog =
        Arc::new(SqliteCatalogStore::open(&library.join("skill-man.sqlite3")).expect("Catalog"));
    let write_gate = Arc::new(WriteGate::open_for_tests());
    let transition = Arc::new(
        SourceTransitionService::new(
            preview.clone(),
            source.clone(),
            locks.clone(),
            catalog.clone(),
            filesystem.clone(),
            Arc::new(FixtureClock),
            library.clone(),
            home.clone(),
            write_gate.clone(),
        )
        .with_update_store(catalog.clone()),
    );
    let lifecycle = Arc::new(SourceLifecycleService::new(
        transition.clone(),
        filesystem.clone(),
        source.clone(),
        Arc::new(FixtureClock),
        library.clone(),
    ));
    let update = Arc::new(SourceUpdateService::new(
        preview.clone(),
        transition.clone(),
        filesystem.clone(),
        library.clone(),
    ));
    Fixture {
        _workspace: workspace,
        repository,
        library,
        home,
        export_root,
        filesystem,
        catalog,
        source,
        locks,
        transition,
        preview,
        lifecycle,
        update,
        lock_path,
        write_gate,
    }
}

fn confirm_transition(fixture: &Fixture) -> String {
    let outcome = fixture
        .preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: FIXTURE_URL.into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("expected clean preview");
    };
    fixture
        .transition
        .confirm(ConfirmSourceTransitionRequest {
            source_type: "git".into(),
            source_url: preview.source_url,
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
            expected_selected_ref: preview.policy.selected_ref,
            expected_resolved_commit: preview.policy.resolved_commit,
        })
        .expect("v9 transition")
        .remote_id
}

fn open_catalog(fixture: &Fixture) -> Connection {
    Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection")
}

fn count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or_else(|error| panic!("count rows in {table}: {error}"))
}

fn member_namespace(fixture: &Fixture, remote_id: &str, skill_path: &str) -> PathBuf {
    let current = read_current(fixture, remote_id);
    let member = current
        .members
        .iter()
        .find(|member| member.skill_path == skill_path)
        .expect("member")
        .clone();
    fixture.library.join(&member.storage_relpath)
}

fn read_current(fixture: &Fixture, remote_id: &str) -> SourceUpdateCurrentSource {
    fixture
        .catalog
        .read_current(remote_id)
        .expect("read current")
        .expect("source")
}

/// The transition assigns stable UUID member ids; tests resolve them by
/// the frozen `skill_path` instead of guessing.
fn member_id_by_path(fixture: &Fixture, remote_id: &str, skill_path: &str) -> String {
    read_current(fixture, remote_id)
        .members
        .iter()
        .find(|member| member.skill_path == skill_path)
        .expect("member")
        .skill_id
        .0
        .clone()
}

fn stored_tree_hash(fixture: &Fixture, remote_id: &str, skill_path: &str) -> String {
    fixture
        .catalog
        .read_current(remote_id)
        .expect("read current")
        .expect("source")
        .members
        .iter()
        .find(|member| member.skill_path == skill_path)
        .expect("member")
        .tree_hash
        .clone()
        .expect("tree hash")
}

/// A SourceUpdateStore that fails `commit_remove_source` exactly once and
/// delegates everything else to the real Catalog.
struct FailOnceRemoveStore {
    inner: Arc<SqliteCatalogStore>,
    fail_once: std::sync::atomic::AtomicBool,
}

impl SourceUpdateStore for FailOnceRemoveStore {
    fn read_current(
        &self,
        remote_id: &str,
    ) -> Result<Option<SourceUpdateCurrentSource>, SourceUpdateStoreError> {
        self.inner.read_current(remote_id)
    }

    fn source_ids(&self) -> Result<Vec<String>, SourceUpdateStoreError> {
        self.inner.source_ids()
    }

    fn member_health(
        &self,
        skill_id: &skill_man_lib::core::domain::SkillId,
    ) -> Result<Option<(String, Health)>, SourceUpdateStoreError> {
        self.inner.member_health(skill_id)
    }

    fn validate_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<(), SourceUpdateStoreError> {
        self.inner.validate_source_update(record)
    }

    fn commit_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        self.inner.commit_source_update(record)
    }

    fn source_update_is_committed(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<bool, SourceUpdateStoreError> {
        self.inner.source_update_is_committed(record)
    }

    fn undo_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        self.inner.undo_source_update(record)
    }

    fn set_source_member_health(
        &self,
        remote_id: &str,
        health: &[(skill_man_lib::core::domain::SkillId, Health)],
    ) -> Result<u64, SourceUpdateStoreError> {
        self.inner.set_source_member_health(remote_id, health)
    }

    fn register_local_copy(
        &self,
        record: &LocalSourceCopyRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        self.inner.register_local_copy(record)
    }

    fn local_copy_is_registered(&self, destination: &Path) -> Result<bool, SourceUpdateStoreError> {
        self.inner.local_copy_is_registered(destination)
    }

    fn source_remove_facts(
        &self,
        remote_id: &str,
    ) -> Result<skill_man_lib::seams::source_update_store::SourceRemoveFacts, SourceUpdateStoreError>
    {
        self.inner.source_remove_facts(remote_id)
    }

    fn commit_remove_source(&self, remote_id: &str) -> Result<u64, SourceUpdateStoreError> {
        if self
            .fail_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(SourceUpdateStoreError::Unavailable(
                "injected Catalog failure before the whole-source Remove commit".into(),
            ));
        }
        self.inner.commit_remove_source(remote_id)
    }

    fn source_remove_is_committed(&self, remote_id: &str) -> Result<bool, SourceUpdateStoreError> {
        self.inner.source_remove_is_committed(remote_id)
    }
}

fn lifecycle_over_store(
    fixture: &Fixture,
    store: Arc<dyn SourceUpdateStore>,
) -> SourceLifecycleService {
    let transition = Arc::new(
        SourceTransitionService::new(
            fixture.preview.clone(),
            fixture.source.clone(),
            fixture.locks.clone(),
            fixture.catalog.clone(),
            fixture.filesystem.clone(),
            Arc::new(FixtureClock),
            fixture.library.clone(),
            fixture.home.clone(),
            fixture.write_gate.clone(),
        )
        .with_update_store(store),
    );
    SourceLifecycleService::new(
        transition,
        fixture.filesystem.clone(),
        fixture.source.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
    )
}

// ---- Whole-source Remove ------------------------------------------------

#[test]
fn remove_source_rolls_back_isolation_when_the_catalog_commit_fails() {
    let mut fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let alpha = member_namespace(&fixture, &remote_id, "skills/alpha");
    let alpha_before = std::fs::read_to_string(alpha.join("SKILL.md")).expect("alpha bytes before");

    let failing = Arc::new(FailOnceRemoveStore {
        inner: fixture.catalog.clone(),
        fail_once: std::sync::atomic::AtomicBool::new(true),
    });
    fixture.catalog = fixture.catalog.clone();
    let lifecycle = lifecycle_over_store(&fixture, failing);
    let result = lifecycle.remove_source(&remote_id);
    assert!(
        matches!(
            result,
            Err(SourceLifecycleError::Store(
                SourceUpdateStoreError::Unavailable(_)
            ))
        ),
        "expected the injected Catalog failure, got {result:?}"
    );

    // Rollback restored every isolated member byte-for-byte.
    let restored = std::fs::read_to_string(alpha.join("SKILL.md")).expect("alpha bytes back");
    assert_eq!(restored, alpha_before);
    assert!(member_namespace(&fixture, &remote_id, "skills/beta").is_dir());
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "git_source_members"), 2);
    assert_eq!(count(&connection, "skills"), 2);
    // The pending journal is closed after a completed rollback.
    let operations = fixture.library.join("operations");
    let pending = std::fs::read_dir(&operations)
        .expect("operations root")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("source-transition-remove-")
        })
        .count();
    assert_eq!(pending, 0, "no pending Remove journal after rollback");
    let _ = &mut fixture;
}

#[test]
fn remove_source_recovers_forward_from_a_catalog_committed_journal() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let operation_id = "source-transition-remove-crash-1";

    // Freeze the crash state: members isolated (namespaces moved aside),
    // journal at CatalogCommitted, Catalog row still present.
    let mut members = Vec::new();
    let mut namespaces = Vec::new();
    for skill_path in ["skills/alpha", "skills/beta"] {
        let namespace = member_namespace(&fixture, &remote_id, skill_path);
        namespaces.push(namespace.clone());
        let observed = fixture
            .filesystem
            .staged_tree_snapshot(&namespace)
            .expect("observed snapshot")
            .content_hash;
        let isolated = fixture
            .filesystem
            .isolate_external_source(&namespace, operation_id)
            .expect("isolate member");
        let member = read_current(&fixture, &remote_id)
            .members
            .iter()
            .find(|member| member.skill_path == skill_path)
            .expect("member")
            .clone();
        members.push(RemoveSourceMemberJournal {
            skill_id: member.skill_id.0.clone(),
            directory_name: member.directory_name.clone(),
            namespace_path: namespace,
            observed_tree_hash: observed,
            isolated_path: Some(isolated),
        });
    }
    let activations: Vec<RemoveSourceActivationJournal> = fixture
        .catalog
        .source_remove_facts(&remote_id)
        .expect("remove facts")
        .activations
        .iter()
        .map(|activation| RemoveSourceActivationJournal {
            skill_id: activation.skill_id.0.clone(),
            entry_path: activation.entry_path.clone(),
            target_path: activation.target_path.clone(),
        })
        .collect();
    fixture
        .filesystem
        .write_source_lifecycle_journal(
            &fixture.library,
            &SourceLifecycleJournal::RemoveSource(RemoveSourceJournal {
                version: 1,
                operation_id: operation_id.into(),
                phase: RemoveSourcePhase::CatalogCommitted,
                remote_id: remote_id.clone(),
                canonical_url: FIXTURE_URL.into(),
                staging_operation_root: fixture.library.join("staging").join(operation_id),
                staging_fingerprint: None,
                members,
                activations,
            }),
        )
        .expect("freeze crash journal");

    fixture
        .lifecycle
        .recover_lifecycle(&fixture.library)
        .expect("roll forward");

    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 0);
    assert_eq!(count(&connection, "git_source_members"), 0);
    assert_eq!(count(&connection, "skills"), 0);
    for namespace in &namespaces {
        assert!(!namespace.exists(), "isolated members stay removed");
    }
    let pending = std::fs::read_dir(fixture.library.join("operations"))
        .expect("operations root")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("source-transition-remove-")
        })
        .count();
    assert_eq!(
        pending, 0,
        "the crash journal is finished after roll-forward"
    );
}

// ---- Restore Current Source Release -------------------------------------

#[test]
fn restore_current_release_restores_drifted_member_bytes_and_health() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let alpha = member_namespace(&fixture, &remote_id, "skills/alpha");
    let release_hash = stored_tree_hash(&fixture, &remote_id, "skills/alpha");

    // The user (or anything else) mutated the immutable snapshot.
    write_file(&alpha, "SKILL.md", "---\nname: Drifted\n---\ndrifted\n");
    let drifted_hash = fixture
        .filesystem
        .staged_tree_snapshot(&alpha)
        .expect("drifted snapshot")
        .content_hash;
    assert_ne!(drifted_hash, release_hash, "fixture drifted the bytes");

    let result = fixture
        .lifecycle
        .restore_current_release(&remote_id)
        .expect("restore succeeds for a fetchable persisted release");
    assert_eq!(result.remote_id, remote_id);
    assert_eq!(result.restored_members, 1);

    let restored = std::fs::read_to_string(alpha.join("SKILL.md")).expect("restored bytes");
    assert_eq!(restored, ALPHA_SKILL_MD, "release bytes are reinstalled");
    let restored_hash = fixture
        .filesystem
        .staged_tree_snapshot(&alpha)
        .expect("restored snapshot")
        .content_hash;
    assert_eq!(restored_hash, release_hash);
    let alpha_id = member_id_by_path(&fixture, &remote_id, "skills/alpha");
    let health: String = open_catalog(&fixture)
        .query_row(
            "SELECT health FROM skills WHERE id = ?1",
            [&alpha_id],
            |row| row.get(0),
        )
        .expect("health row");
    assert_eq!(
        health, "healthy",
        "Restore flips the member back to Healthy"
    );
}

#[test]
fn restore_rolls_back_isolated_observed_bytes_from_a_planned_journal() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let alpha = member_namespace(&fixture, &remote_id, "skills/alpha");
    let current = fixture
        .catalog
        .read_current(&remote_id)
        .expect("read current")
        .expect("source");
    let member = current
        .members
        .iter()
        .find(|member| member.skill_path == "skills/alpha")
        .expect("alpha member")
        .clone();

    // Crash state: the drifted observed bytes were isolated (namespace
    // absent), the journal frozen at BackedUp — before any install.
    write_file(&alpha, "SKILL.md", "---\nname: Drifted\n---\ndrifted\n");
    let observed_hash = fixture
        .filesystem
        .staged_tree_snapshot(&alpha)
        .expect("drifted snapshot")
        .content_hash;
    let operation_id = "source-transition-restore-crash-1";
    let backup = fixture
        .filesystem
        .isolate_external_source(&alpha, operation_id)
        .expect("isolate drifted bytes");
    assert!(!alpha.exists(), "isolation removes the namespace");
    fixture
        .filesystem
        .write_source_lifecycle_journal(
            &fixture.library,
            &SourceLifecycleJournal::Restore(RestoreSourceJournal {
                version: 1,
                operation_id: operation_id.into(),
                phase: RestoreSourcePhase::BackedUp,
                remote_id: remote_id.clone(),
                release_id: current.current_release_id.clone(),
                resolved_commit: current.resolved_commit.clone(),
                staging_operation_root: fixture.library.join("staging").join(operation_id),
                staging_fingerprint: None,
                members: vec![RestoreSourceMember {
                    skill_id: member.skill_id.0.clone(),
                    directory_name: "alpha".into(),
                    skill_path: "skills/alpha".into(),
                    namespace_path: alpha.clone(),
                    staged_root: fixture
                        .library
                        .join("staging")
                        .join(operation_id)
                        .join("alpha"),
                    release_tree_hash: member.tree_hash.clone().unwrap_or_default(),
                    observed_tree_hash: observed_hash,
                    staged_snapshot: None,
                    backup_path: Some(backup),
                    restored: false,
                }],
            }),
        )
        .expect("freeze crash journal");

    fixture
        .lifecycle
        .recover_lifecycle(&fixture.library)
        .expect("roll back");

    let restored = std::fs::read_to_string(alpha.join("SKILL.md")).expect("observed bytes back");
    assert_eq!(
        restored, "---\nname: Drifted\n---\ndrifted\n",
        "rollback restores the exact observed bytes, never release bytes"
    );
    let pending = std::fs::read_dir(fixture.library.join("operations"))
        .expect("operations root")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("source-transition-restore-")
        })
        .count();
    assert_eq!(pending, 0, "the rolled-back journal is finished");
}

// ---- Create Local Source Copy -------------------------------------------

#[test]
fn create_local_copy_happy_path_registers_and_recovery_is_idempotent() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let destination = fixture.export_root.join("alpha-copy");

    fixture
        .lifecycle
        .create_local_copy(
            &remote_id,
            &member_id_by_path(&fixture, &remote_id, "skills/alpha"),
            &destination,
        )
        .expect("local copy succeeds");
    let copied = std::fs::read_to_string(destination.join("SKILL.md")).expect("copied bytes");
    assert_eq!(copied, ALPHA_SKILL_MD, "observed member bytes are copied");
    // The original member namespace and Activation stay untouched.
    assert!(member_namespace(&fixture, &remote_id, "skills/alpha").is_dir());
    assert!(
        !destination.join(".git").exists(),
        "no .git in a Local Source"
    );

    // Crash after registration: a frozen Registered journal with the
    // destination present converges without touching the bytes again.
    let content_hash = fixture
        .filesystem
        .staged_tree_snapshot(&destination)
        .expect("copied snapshot")
        .content_hash;
    let operation_id = "source-transition-local-copy-crash-1";
    fixture
        .filesystem
        .write_source_lifecycle_journal(
            &fixture.library,
            &SourceLifecycleJournal::LocalCopy(LocalCopyJournal {
                version: 1,
                operation_id: operation_id.into(),
                phase: LocalCopyPhase::Registered,
                remote_id: remote_id.clone(),
                skill_id: member_id_by_path(&fixture, &remote_id, "skills/alpha"),
                directory_name: "alpha".into(),
                identity_key: skill_man_lib::core::domain::skill_identity_key("alpha"),
                display_name: "Alpha".into(),
                description: String::new(),
                source_path: member_namespace(&fixture, &remote_id, "skills/alpha"),
                destination: destination.clone(),
                staged_path: fixture.export_root.join(".staging-never"),
                content_hash,
            }),
        )
        .expect("freeze crash journal");
    fixture
        .lifecycle
        .recover_lifecycle(&fixture.library)
        .expect("registered Local Copy converges");
    let still = std::fs::read_to_string(destination.join("SKILL.md")).expect("bytes unchanged");
    assert_eq!(still, ALPHA_SKILL_MD);
}

#[test]
fn create_local_copy_recovery_blocks_when_the_registered_destination_vanished() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let destination = fixture.export_root.join("vanishing-copy");
    let content_hash = "sha256:absent";

    let operation_id = "source-transition-local-copy-crash-2";
    fixture
        .filesystem
        .write_source_lifecycle_journal(
            &fixture.library,
            &SourceLifecycleJournal::LocalCopy(LocalCopyJournal {
                version: 1,
                operation_id: operation_id.into(),
                phase: LocalCopyPhase::Registered,
                remote_id: remote_id.clone(),
                skill_id: member_id_by_path(&fixture, &remote_id, "skills/alpha"),
                directory_name: "alpha".into(),
                identity_key: skill_man_lib::core::domain::skill_identity_key("alpha"),
                display_name: "Alpha".into(),
                description: String::new(),
                source_path: member_namespace(&fixture, &remote_id, "skills/alpha"),
                destination: destination.clone(),
                staged_path: fixture.export_root.join(".staging-never"),
                content_hash: content_hash.into(),
            }),
        )
        .expect("freeze crash journal");
    assert!(!destination.exists(), "the registered copy is missing");

    let result = fixture.lifecycle.recover_lifecycle(&fixture.library);
    assert!(
        matches!(result, Err(SourceLifecycleError::RecoveryRequired(_))),
        "a vanished registered Local Source must block for recovery, got {result:?}"
    );
}

#[test]
fn tampered_local_copy_recovery_keeps_an_external_destination_untouched() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let member = read_current(&fixture, &remote_id)
        .members
        .into_iter()
        .find(|member| member.skill_path == "skills/alpha")
        .expect("alpha member");
    let source_path = member_namespace(&fixture, &remote_id, "skills/alpha");
    let destination = fixture.export_root.join("tampered-destination");
    std::fs::create_dir_all(&destination).expect("create protected destination");
    std::fs::write(destination.join("keep.txt"), "keep").expect("write protected content");
    let operation_id = "source-transition-local-copy-tampered";
    let staged_path = fixture
        .export_root
        .join(format!(".skill-man-source-transition-{operation_id}-alpha"));
    let content_hash = fixture
        .filesystem
        .staged_tree_snapshot(&source_path)
        .expect("source snapshot")
        .content_hash;
    fixture
        .filesystem
        .write_source_lifecycle_journal(
            &fixture.library,
            &SourceLifecycleJournal::LocalCopy(LocalCopyJournal {
                version: 1,
                operation_id: operation_id.into(),
                phase: LocalCopyPhase::Copied,
                remote_id,
                skill_id: member.skill_id.0,
                directory_name: member.directory_name,
                identity_key: member.identity_key,
                display_name: member.display_name,
                description: member.description,
                source_path,
                destination: destination.clone(),
                staged_path,
                content_hash,
            }),
        )
        .expect("write tampered journal");

    let result = fixture.lifecycle.recover_lifecycle(&fixture.library);
    assert!(
        matches!(result, Err(SourceLifecycleError::RecoveryRequired(_))),
        "tampered journal must keep startup recovery locked, got {result:?}"
    );
    assert!(
        destination.join("keep.txt").is_file(),
        "unsafe destination must never be deleted"
    );
}

#[test]
fn create_local_copy_rejects_destinations_inside_the_home() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let destination = fixture.library.join("skills").join("outside-attack");

    let result = fixture.lifecycle.create_local_copy(
        &remote_id,
        &member_id_by_path(&fixture, &remote_id, "skills/alpha"),
        &destination,
    );
    assert!(
        matches!(result, Err(SourceLifecycleError::Validation(_))),
        "a destination inside Home must be rejected, got {result:?}"
    );
}

// ---- Update gate ---------------------------------------------------------

#[test]
fn confirm_update_reports_ownership_conflict_when_the_installer_reappears() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);

    // The external installer reappears with the same repository claim.
    std::fs::write(
        &fixture.lock_path,
        r#"{
  "version": 3,
  "skills": {
    "alpha": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/alpha",
      "skillFolderHash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    },
    "beta": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/beta",
      "skillFolderHash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  }
}"#,
    )
    .expect("reappear external claim");

    // confirm_update probes external claims before refreshing the preview,
    // so fixed expected refs never reach the stale check.
    let error = fixture
        .update
        .confirm(&remote_id, None, "main".into(), "0".into())
        .expect_err("the reappeared installer must conflict with Update");
    assert!(
        matches!(
            error,
            SourceUpdateError::Transition(SourceTransitionError::ExternalOwnershipReappeared)
        ),
        "expected Ownership Conflict, got {error:?}"
    );
}

#[test]
fn verify_all_members_flags_drift_and_blocks_update_until_restored() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let alpha = member_namespace(&fixture, &remote_id, "skills/alpha");
    write_file(&alpha, "SKILL.md", "---\nname: Drifted\n---\ndrifted\n");

    let mismatched = fixture.update.verify_all_members().expect("verify");
    assert_eq!(mismatched, 1, "alpha drifted");
    let alpha_id = member_id_by_path(&fixture, &remote_id, "skills/alpha");
    let (_, health) = fixture
        .catalog
        .member_health(&skill_man_lib::core::domain::SkillId(alpha_id.clone()))
        .expect("member health")
        .expect("alpha health");
    assert_eq!(health, Health::SourceSnapshotMismatch);

    // Blocked: Update refuses while the snapshot mismatches the release.
    let error = fixture
        .update
        .confirm(&remote_id, None, "main".into(), "0".into())
        .expect_err("mismatch blocks Update");
    assert!(
        matches!(
            error,
            SourceUpdateError::Transition(SourceTransitionError::SourceSnapshotMismatch)
                | SourceUpdateError::SourceSnapshotMismatch
        ),
        "expected SourceSnapshotMismatch, got {error:?}"
    );

    // Restore clears the mismatch.
    fixture
        .lifecycle
        .restore_current_release(&remote_id)
        .expect("restore");
    let mismatched = fixture.update.verify_all_members().expect("verify");
    assert_eq!(mismatched, 0);
    let (_, health) = fixture
        .catalog
        .member_health(&skill_man_lib::core::domain::SkillId(alpha_id))
        .expect("member health")
        .expect("alpha health");
    assert_eq!(health, Health::Healthy);
}

// ---- Update policy reuse, reappear, new-Enable gate and Remove gating ---

#[test]
fn update_reuses_persisted_policy_and_reappearing_members_recover() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let beta_id = member_id_by_path(&fixture, &remote_id, "skills/beta");

    // An Update without an explicit override re-evaluates the source's
    // persisted branch policy instead of falling back to the auto default.
    let draft = fixture
        .update
        .preview(&remote_id, None)
        .expect("update preview reuses the persisted policy");
    assert_eq!(draft.policy.mode, "branch");
    assert_eq!(draft.policy.value.as_deref(), Some("main"));

    // The remote drops beta: the update must tombstone it.
    git(&fixture.repository, &["rm", "-r", "-q", "skills/beta"]);
    git(
        &fixture.repository,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "drop beta",
        ],
    );
    let draft = fixture
        .update
        .preview(&remote_id, None)
        .expect("update preview after drop");
    let beta = draft
        .members
        .iter()
        .find(|member| member.skill_path == "skills/beta")
        .expect("beta in draft");
    assert_eq!(beta.state, SourceUpdateMemberState::Removed);
    assert_eq!(beta.skill_id, beta_id, "removal keeps the stable id");
    let tombstone_result = fixture
        .update
        .confirm(
            &remote_id,
            None,
            draft.policy.selected_ref.clone(),
            draft.policy.resolved_commit.clone(),
        )
        .expect("confirm tombstone");
    let after = read_current(&fixture, &remote_id);
    let beta = after
        .members
        .iter()
        .find(|member| member.skill_path == "skills/beta")
        .expect("beta tombstone row");
    assert_eq!(beta.presence, SourceMemberPresence::Absent);
    assert_eq!(beta.health, Health::Broken);
    assert_eq!(beta.skill_id.0, beta_id);
    assert!(
        !fixture.library.join(&beta.storage_relpath).exists(),
        "the vanished snapshot is deleted"
    );
    // New-Enable gate refuses a tombstoned member.
    let error = fixture
        .update
        .ensure_new_enable_allowed(&SkillId(beta_id.clone()))
        .expect_err("tombstoned member cannot be enabled");
    assert!(
        matches!(error, SourceUpdateError::Validation(_)),
        "expected tombstone Validation, got {error:?}"
    );

    // Service-level Update Undo restores the exact previous state.
    fixture
        .update
        .undo(&tombstone_result.operation_id)
        .expect("source Update Undo");
    let after_undo = read_current(&fixture, &remote_id);
    let beta = after_undo
        .members
        .iter()
        .find(|member| member.skill_path == "skills/beta")
        .expect("beta after undo");
    assert_eq!(beta.presence, SourceMemberPresence::Current);
    assert_eq!(beta.health, Health::Healthy);
    assert_eq!(beta.skill_id.0, beta_id);

    // beta reappears at the same skillPath: the original stable id and
    // storage path are reused and health recovers to Healthy.
    write_file(
        &fixture.repository,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\n---\n# Beta v2\n",
    );
    git(&fixture.repository, &["add", "-A"]);
    git(
        &fixture.repository,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "reappear beta",
        ],
    );
    let draft = fixture
        .update
        .preview(&remote_id, None)
        .expect("update preview after reappear");
    let beta = draft
        .members
        .iter()
        .find(|member| member.skill_path == "skills/beta")
        .expect("beta reappeared");
    assert_eq!(
        beta.skill_id, beta_id,
        "reappearance reuses the stable skill_id"
    );
    fixture
        .update
        .confirm(
            &remote_id,
            None,
            draft.policy.selected_ref.clone(),
            draft.policy.resolved_commit.clone(),
        )
        .expect("confirm reappear");
    let after = read_current(&fixture, &remote_id);
    let beta = after
        .members
        .iter()
        .find(|member| member.skill_path == "skills/beta")
        .expect("beta current");
    assert_eq!(beta.presence, SourceMemberPresence::Current);
    assert_eq!(beta.health, Health::Healthy);
    assert_eq!(beta.skill_id.0, beta_id);
    assert!(
        fixture.library.join(&beta.storage_relpath).is_dir(),
        "the namespace is republished at the original storage path"
    );
    fixture
        .update
        .ensure_new_enable_allowed(&SkillId(beta_id))
        .expect("reappeared member passes the new-Enable gate");
}

#[test]
fn ensure_new_enable_allowed_rejects_mismatch_and_passes_for_non_git() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let alpha_id = member_id_by_path(&fixture, &remote_id, "skills/alpha");
    fixture
        .update
        .ensure_new_enable_allowed(&SkillId(alpha_id.clone()))
        .expect("healthy member passes the new-Enable gate");
    // Unknown / non-Git skill ids have no snapshot gate.
    fixture
        .update
        .ensure_new_enable_allowed(&SkillId("unknown-skill".into()))
        .expect("non-Git skills are not gated");

    let alpha = member_namespace(&fixture, &remote_id, "skills/alpha");
    write_file(&alpha, "SKILL.md", "---\nname: Drifted\n---\ndrifted\n");
    fixture.update.verify_all_members().expect("verify drift");
    let error = fixture
        .update
        .ensure_new_enable_allowed(&SkillId(alpha_id))
        .expect_err("mismatch blocks new Enable");
    assert!(
        matches!(error, SourceUpdateError::SourceSnapshotMismatch),
        "expected SourceSnapshotMismatch, got {error:?}"
    );
}

#[test]
fn remove_source_is_refused_under_snapshot_mismatch() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let alpha = member_namespace(&fixture, &remote_id, "skills/alpha");
    write_file(&alpha, "SKILL.md", "drifted bytes");
    fixture.update.verify_all_members().expect("verify drift");
    let error = fixture
        .lifecycle
        .remove_source(&remote_id)
        .expect_err("Remove must be refused under Snapshot Mismatch");
    assert!(
        matches!(error, SourceLifecycleError::SourceSnapshotMismatch),
        "expected SourceSnapshotMismatch, got {error:?}"
    );
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "skills"), 2);
}

// ---- Lifecycle crash recovery at the remaining commit points ------------

#[test]
fn local_copy_rolls_back_a_pre_commit_crash_journal() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let alpha = member_namespace(&fixture, &remote_id, "skills/alpha");
    let destination = fixture.export_root.join("copy-alpha");
    let operation_id = "source-transition-local-copy-crash-1";
    // The real isolation staging name (hidden handoff copy contract).
    let staged = fixture
        .export_root
        .join(format!(".skill-man-source-transition-{operation_id}-alpha"));
    let snapshot = fixture
        .filesystem
        .staged_tree_snapshot(&alpha)
        .expect("alpha bytes");
    fixture
        .filesystem
        .copy_tree_verified(&alpha, &staged)
        .expect("staged bytes");
    let current = read_current(&fixture, &remote_id);
    let member = current
        .members
        .iter()
        .find(|member| member.skill_path == "skills/alpha")
        .expect("alpha member")
        .clone();
    fixture
        .filesystem
        .write_source_lifecycle_journal(
            &fixture.library,
            &SourceLifecycleJournal::LocalCopy(LocalCopyJournal {
                version: 1,
                operation_id: operation_id.into(),
                phase: LocalCopyPhase::Copied,
                remote_id: remote_id.clone(),
                skill_id: member.skill_id.0.clone(),
                directory_name: member.directory_name.clone(),
                identity_key: member.identity_key.clone(),
                display_name: member.display_name.clone(),
                description: member.description.clone(),
                source_path: alpha,
                destination: destination.clone(),
                staged_path: staged.clone(),
                content_hash: snapshot.content_hash.clone(),
            }),
        )
        .expect("freeze crash journal");
    fixture
        .lifecycle
        .recover_lifecycle(&fixture.library)
        .expect("pre-commit Local Copy crash rolls back");
    assert!(!staged.exists(), "the staged copy is discarded");
    assert!(
        !destination.exists(),
        "no half-copy destination after rollback"
    );
    let connection = open_catalog(&fixture);
    assert_eq!(
        count(&connection, "skills"),
        2,
        "no Local Source registered"
    );
    let operations = fixture.library.join("operations");
    let pending = std::fs::read_dir(&operations)
        .expect("operations root")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("source-transition-local-copy-")
        })
        .count();
    assert_eq!(pending, 0, "the crash journal is closed after rollback");
}

#[test]
fn restore_roll_forward_commits_member_health_after_a_restored_phase_crash() {
    let fixture = fixture();
    let remote_id = confirm_transition(&fixture);
    let alpha = member_namespace(&fixture, &remote_id, "skills/alpha");
    let current = read_current(&fixture, &remote_id);
    let member = current
        .members
        .iter()
        .find(|member| member.skill_path == "skills/alpha")
        .expect("alpha member")
        .clone();
    let alpha_id = member.skill_id.0.clone();
    // The crash happened after the bytes were installed (journal at
    // Restored) but before the catalog health commit: health is stale.
    fixture
        .catalog
        .set_source_member_health(
            &remote_id,
            &[(SkillId(alpha_id.clone()), Health::SourceSnapshotMismatch)],
        )
        .expect("health mismatch");
    let operation_id = "source-transition-restore-crash-1";
    let staging_operation_root = fixture.library.join("staging").join(operation_id);
    let fingerprint = fixture
        .filesystem
        .create_adopt_staging_operation(&fixture.library, operation_id)
        .expect("staging operation");
    let release_hash = stored_tree_hash(&fixture, &remote_id, "skills/alpha");
    fixture
        .filesystem
        .write_source_lifecycle_journal(
            &fixture.library,
            &SourceLifecycleJournal::Restore(RestoreSourceJournal {
                version: 1,
                operation_id: operation_id.into(),
                phase: RestoreSourcePhase::Restored,
                remote_id: remote_id.clone(),
                release_id: current.current_release_id.clone(),
                resolved_commit: current.resolved_commit.clone(),
                staging_operation_root,
                staging_fingerprint: Some(fingerprint),
                members: vec![RestoreSourceMember {
                    skill_id: alpha_id.clone(),
                    directory_name: member.directory_name.clone(),
                    skill_path: member.skill_path.clone(),
                    namespace_path: alpha.clone(),
                    staged_root: fixture
                        .library
                        .join("staging")
                        .join(operation_id)
                        .join(&member.directory_name),
                    release_tree_hash: release_hash,
                    observed_tree_hash: "mismatch-observed".into(),
                    staged_snapshot: None,
                    backup_path: None,
                    restored: true,
                }],
            }),
        )
        .expect("freeze restored journal");
    fixture
        .lifecycle
        .recover_lifecycle(&fixture.library)
        .expect("Restored-phase crash rolls forward");
    let (_, health) = fixture
        .catalog
        .member_health(&SkillId(alpha_id))
        .expect("member health")
        .expect("alpha health");
    assert_eq!(
        health,
        Health::Healthy,
        "roll-forward also commits the catalog health flip"
    );
}
