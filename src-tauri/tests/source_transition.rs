use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rusqlite::Connection;
use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreviewOutcome, SourceGroupPreviewService,
};
use skill_man_lib::core::source_transition::{
    ConfirmSourceTransitionRequest, SourceTransitionError, SourceTransitionService,
};
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::seams::installer_lock_store::{
    InstallerLockError, InstallerLockStore, LockFileReport,
};
use skill_man_lib::seams::source::{GitFetchReport, GitSource, GitTreeEntry, SourceError};
use skill_man_lib::seams::source_transition_store::{
    SourceTransitionRecord, SourceTransitionStore, SourceTransitionStoreError,
};

struct FixtureGitSource {
    inner: SystemGitSource,
    fixture_url: String,
}

/// Post-CAS recovery must use only the frozen journal and must never touch
/// the remote again, even to resolve the already-fixed commit.
struct NoFetchGitSource;

impl GitSource for NoFetchGitSource {
    fn fetch_mirror(&self, _url: &str, _mirror_dir: &Path) -> Result<GitFetchReport, SourceError> {
        Err(SourceError::Git("recovery must not fetch Git".into()))
    }

    fn resolve_commit(
        &self,
        _mirror_dir: &Path,
        _rev: &str,
    ) -> Result<Option<String>, SourceError> {
        Err(SourceError::Git("recovery must not resolve Git".into()))
    }

    fn list_tree(
        &self,
        _mirror_dir: &Path,
        _commit: &str,
    ) -> Result<Vec<GitTreeEntry>, SourceError> {
        Err(SourceError::Git("recovery must not list Git".into()))
    }

    fn read_blob(
        &self,
        _mirror_dir: &Path,
        _commit: &str,
        _path: &str,
        _max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, SourceError> {
        Err(SourceError::Git("recovery must not read Git".into()))
    }

    fn tree_summary(
        &self,
        _mirror_dir: &Path,
        _commit: &str,
        _skill_path: &str,
    ) -> Result<String, SourceError> {
        Err(SourceError::Git("recovery must not summarize Git".into()))
    }

    fn stage_skill(
        &self,
        _mirror_dir: &Path,
        _commit: &str,
        _skill_path: &str,
        _destination: &Path,
    ) -> Result<(), SourceError> {
        Err(SourceError::Git("recovery must not stage Git".into()))
    }
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
    fn fetch_mirror(&self, _url: &str, mirror_dir: &Path) -> Result<GitFetchReport, SourceError> {
        self.inner.fetch_mirror(&self.fixture_url, mirror_dir)
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

struct FailOnceSourceStore {
    inner: Arc<SqliteCatalogStore>,
    fail_commit_once: AtomicBool,
}

/// Simulates a crash after the Catalog's atomic Source Undo, before any Home
/// or external restoration begins.
struct FailOnceUndoSourceStore {
    inner: Arc<SqliteCatalogStore>,
    fail_undo_once: AtomicBool,
}

impl FailOnceUndoSourceStore {
    fn new(inner: Arc<SqliteCatalogStore>) -> Self {
        Self {
            inner,
            fail_undo_once: AtomicBool::new(true),
        }
    }
}

impl SourceTransitionStore for FailOnceUndoSourceStore {
    fn validate_new_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<(), SourceTransitionStoreError> {
        self.inner.validate_new_source_transition(record)
    }

    fn commit_source_transition(
        &self,
        record: SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        self.inner.commit_source_transition(record)
    }

    fn source_transition_is_committed(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<bool, SourceTransitionStoreError> {
        self.inner.source_transition_is_committed(record)
    }

    fn undo_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        let snapshot = self.inner.undo_source_transition(record)?;
        if self.fail_undo_once.swap(false, Ordering::SeqCst) {
            return Err(SourceTransitionStoreError::Unavailable(
                "injected crash after Catalog Source Undo".into(),
            ));
        }
        Ok(snapshot)
    }
}

/// Injects an external path occupant between Undo's first preflight and the
/// mandatory just-before-Catalog-mutation revalidation.
struct OccupyOnSecondDiscoverLockStore {
    inner: Arc<SystemInstallerLockStore>,
    path: PathBuf,
    discover_calls: AtomicUsize,
}

impl InstallerLockStore for OccupyOnSecondDiscoverLockStore {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        if self.discover_calls.fetch_add(1, Ordering::SeqCst) == 1 {
            std::fs::write(&self.path, "racing external owner")
                .expect("occupy the external path between Undo guards");
        }
        self.inner.discover()
    }
}

impl FailOnceSourceStore {
    fn new(inner: Arc<SqliteCatalogStore>, fail_commit_once: bool) -> Self {
        Self {
            inner,
            fail_commit_once: AtomicBool::new(fail_commit_once),
        }
    }
}

impl SourceTransitionStore for FailOnceSourceStore {
    fn validate_new_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<(), SourceTransitionStoreError> {
        self.inner.validate_new_source_transition(record)
    }

    fn commit_source_transition(
        &self,
        record: SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        if self.fail_commit_once.swap(false, Ordering::SeqCst) {
            return Err(SourceTransitionStoreError::Unavailable(
                "injected post-CAS Catalog failure".into(),
            ));
        }
        self.inner.commit_source_transition(record)
    }

    fn source_transition_is_committed(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<bool, SourceTransitionStoreError> {
        self.inner.source_transition_is_committed(record)
    }

    fn undo_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        self.inner.undo_source_transition(record)
    }
}

fn git(repository: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
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
    write_file(
        &repository,
        "skills/source/SKILL.md",
        "---\nname: Source Root\ndescription: Root member\n---\n# Root\n",
    );
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

struct Fixture {
    _workspace: tempfile::TempDir,
    home: PathBuf,
    library: PathBuf,
    preview: Arc<SourceGroupPreviewService>,
    source: Arc<dyn GitSource>,
    locks: Arc<SystemInstallerLockStore>,
    filesystem: Arc<MacOsFileSystem>,
    catalog: Arc<SqliteCatalogStore>,
}

fn fixture() -> Fixture {
    let workspace = tempfile::tempdir().expect("workspace");
    let repository = fixture_repository(workspace.path());
    let home = workspace.path().join("home");
    let library = workspace.path().join("library");
    std::fs::create_dir_all(&library).expect("Library");
    let source_root = home.join(".agents/skills");
    write_file(
        &source_root.join("source"),
        "SKILL.md",
        "---\nname: Source Root\ndescription: Root member\n---\n# Root\n",
    );
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
    "source": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/source",
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
    Fixture {
        _workspace: workspace,
        home,
        library,
        preview,
        source,
        locks,
        filesystem,
        catalog,
    }
}

fn confirmation(fixture: &Fixture) -> ConfirmSourceTransitionRequest {
    let outcome = fixture
        .preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_ref: Some("main".into()),
        })
        .expect("preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("expected clean preview");
    };
    assert_eq!(preview.members.len(), 2);
    ConfirmSourceTransitionRequest {
        source_type: "git".into(),
        source_url: preview.source_url,
        tracking_ref: preview.tracking_ref,
        expected_resolved_commit: preview.resolved_commit,
    }
}

fn service(fixture: &Fixture, store: Arc<dyn SourceTransitionStore>) -> SourceTransitionService {
    service_with_locks(fixture, fixture.locks.clone(), store)
}

fn service_with_locks(
    fixture: &Fixture,
    locks: Arc<dyn InstallerLockStore>,
    store: Arc<dyn SourceTransitionStore>,
) -> SourceTransitionService {
    SourceTransitionService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        locks,
        store,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
}

#[test]
fn confirms_and_undoes_the_complete_source_in_one_release() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("confirm source");

    assert_eq!(result.member_count, 2);
    assert!(fixture.library.join("skills/source/SKILL.md").is_file());
    assert!(fixture.library.join("skills/beta/SKILL.md").is_file());
    assert!(!fixture.home.join(".agents/skills/source").exists());
    assert!(!fixture.home.join(".agents/skills/beta").exists());
    let reports = fixture.locks.discover().expect("read released lock");
    assert!(
        reports[0].entries.is_empty(),
        "one CAS released both claims"
    );
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let release_members: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM git_source_release_members",
            [],
            |row| row.get(0),
        )
        .expect("release members");
    let current_members: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_source_members", [], |row| {
            row.get(0)
        })
        .expect("current members");
    assert_eq!((release_members, current_members), (2, 2));
    assert_eq!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("result journal")
            .len(),
        1
    );

    let undo = transition.undo(&result.operation_id).expect("Source Undo");
    assert_eq!(undo.member_count, 2);
    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
    assert!(fixture.home.join(".agents/skills/beta/SKILL.md").is_file());
    assert!(!fixture.library.join("skills/source").exists());
    assert!(!fixture.library.join("skills/beta").exists());
    let reports = fixture.locks.discover().expect("read restored lock");
    assert_eq!(
        reports[0].entries.len(),
        2,
        "one restore recovered both claims"
    );
    let remaining_sources: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("source count");
    assert_eq!(remaining_sources, 0);
}

#[test]
fn post_cas_failure_recovers_only_the_frozen_journal_without_refetching() {
    let fixture = fixture();
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let transition = service(&fixture, flaky.clone());
    let error = transition
        .confirm(confirmation(&fixture))
        .expect_err("injected Catalog failure must require recovery");
    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("pending journal")
            .len()
            == 1
    );
    assert!(
        fixture.locks.discover().expect("lock")[0]
            .entries
            .is_empty()
    );

    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    );
    recovery
        .recover_pending(&fixture.library)
        .expect("recover frozen release");
    assert!(fixture.library.join("skills/source/SKILL.md").is_file());
    assert!(fixture.library.join("skills/beta/SKILL.md").is_file());
    assert!(!fixture.home.join(".agents/skills/source").exists());
    assert!(!fixture.home.join(".agents/skills/beta").exists());
    assert_eq!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("completed journal archived")
            .len(),
        0
    );
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let member_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_source_members", [], |row| {
            row.get(0)
        })
        .expect("members");
    assert_eq!(member_count, 2);
}

#[test]
fn confirmation_refuses_a_home_destination_before_releasing_external_ownership() {
    let fixture = fixture();
    write_file(
        &fixture.library.join("skills/source"),
        "SKILL.md",
        "# Existing Home occupant\n",
    );
    let transition = service(&fixture, fixture.catalog.clone());

    let error = transition
        .confirm(confirmation(&fixture))
        .expect_err("a Home destination collision is pre-CAS stale evidence");

    assert!(matches!(error, SourceTransitionError::PreviewStale));
    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
    assert!(fixture.home.join(".agents/skills/beta/SKILL.md").is_file());
    assert_eq!(
        fixture.locks.discover().expect("lock after rejection")[0]
            .entries
            .len(),
        2,
        "the one source lock remains untouched before its CAS",
    );
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("journal cleanup")
            .is_empty()
    );
}

#[test]
fn undo_rechecks_external_occupancy_before_mutating_the_catalog() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("confirm source");
    let racing_locks: Arc<dyn InstallerLockStore> = Arc::new(OccupyOnSecondDiscoverLockStore {
        inner: fixture.locks.clone(),
        path: fixture.home.join(".agents/skills/source"),
        discover_calls: AtomicUsize::new(0),
    });
    let undo = service_with_locks(&fixture, racing_locks, fixture.catalog.clone());

    let error = undo
        .undo(&result.operation_id)
        .expect_err("the second Undo guard must see the racing external owner");

    assert!(matches!(error, SourceTransitionError::Validation(_)));
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let source_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("source count");
    assert_eq!(
        source_count, 1,
        "Undo did not delete the whole source first"
    );
    assert!(fixture.library.join("skills/source/SKILL.md").is_file());
    assert!(
        fixture.locks.discover().expect("released lock")[0]
            .entries
            .is_empty(),
        "Undo did not restore only the lock after a failed guard"
    );
}

#[test]
fn recovery_refuses_a_new_repository_claim_under_a_different_lock_key() {
    let fixture = fixture();
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let transition = service(&fixture, flaky.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect_err("leave a post-CAS journal for recovery");
    std::fs::write(
        fixture.home.join(".agents/.skill-lock.json"),
        r#"{
  "version": 3,
  "skills": {
    "replacement": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/replacement",
      "skillFolderHash": "cccccccccccccccccccccccccccccccccccccccc"
    }
  }
}"#,
    )
    .expect("simulate a new external repository claim");
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    );

    let error = recovery
        .recover_pending(&fixture.library)
        .expect_err("a new source claim is never accepted as a released claim");

    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("pending journal remains")
            .len()
            == 1
    );
}

#[test]
fn recovery_refuses_a_frozen_claim_replaced_under_its_original_key() {
    let fixture = fixture();
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let transition = service(&fixture, flaky.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect_err("leave a post-CAS journal for recovery");
    std::fs::write(
        fixture.home.join(".agents/.skill-lock.json"),
        r#"{
  "version": 3,
  "skills": {
    "source": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/source",
      "skillFolderHash": "cccccccccccccccccccccccccccccccccccccccc"
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
    .expect("replace one frozen claim under the same key");
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    );

    let error = recovery
        .recover_pending(&fixture.library)
        .expect_err("a same-key claim replacement is not the frozen pre-CAS state");

    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
}

#[test]
fn undo_recovery_converges_after_catalog_delete_and_partial_external_restore() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("confirm source");
    let crash_store: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceUndoSourceStore::new(fixture.catalog.clone()));
    let undo = service(&fixture, crash_store);
    undo.undo(&result.operation_id)
        .expect_err("simulate a crash after the Catalog Source Undo");

    let journal = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .expect("Undoing journal")
        .into_iter()
        .next()
        .expect("one pending journal");
    let source = journal
        .members
        .iter()
        .find(|member| member.directory_name == "source")
        .expect("source member");
    fixture
        .filesystem
        .remove_directory_verified(&source.final_entity_path)
        .expect("simulate the first Home removal before crash");
    fixture
        .filesystem
        .restore_isolated_source(
            source
                .isolated_path
                .as_ref()
                .expect("source preservation copy"),
            &source.canonical_entity,
            &source.tree_hash,
        )
        .expect("simulate the first external restore before crash");

    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    );
    recovery
        .recover_pending(&fixture.library)
        .expect("complete the frozen partial Source Undo");

    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
    assert!(fixture.home.join(".agents/skills/beta/SKILL.md").is_file());
    assert!(!fixture.library.join("skills/source").exists());
    assert!(!fixture.library.join("skills/beta").exists());
    assert_eq!(
        fixture.locks.discover().expect("restored lock")[0]
            .entries
            .len(),
        2
    );
}

#[test]
fn recovery_refuses_a_journal_isolation_path_outside_the_derived_source_root() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("confirm source");
    let journal_path = fixture
        .library
        .join("operations")
        .join(&result.operation_id)
        .join("source-transition-journal.json");
    let forged = fixture
        .library
        .parent()
        .expect("workspace")
        .join(".skill-man-source-transition-forged");
    std::fs::create_dir_all(&forged).expect("forged isolation directory");
    std::fs::write(forged.join("keep"), "must not be deleted").expect("forged content");
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal_path).expect("read journal"))
            .expect("journal JSON");
    journal["members"][0]["isolated_path"] =
        serde_json::Value::String(forged.to_string_lossy().into_owned());
    std::fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&journal).expect("rewrite forged journal"),
    )
    .expect("write forged journal");
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    );

    let error = recovery
        .recover_pending(&fixture.library)
        .expect_err("journal-owned isolation paths must be exact");

    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
    assert!(
        forged.join("keep").is_file(),
        "forged path was never touched"
    );
}

#[test]
fn journal_writes_refuse_a_symlinked_operations_root() {
    let fixture = fixture();
    let outside = fixture.library.parent().expect("workspace").join("outside");
    std::fs::create_dir_all(&outside).expect("outside directory");
    std::os::unix::fs::symlink(&outside, fixture.library.join("operations"))
        .expect("replace owned operations root with a symlink");
    let transition = service(&fixture, fixture.catalog.clone());

    transition
        .confirm(confirmation(&fixture))
        .expect_err("journal writer must reject the symlink before any transition work");

    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
    assert_eq!(
        fixture.locks.discover().expect("lock after refusal")[0]
            .entries
            .len(),
        2
    );
    assert!(
        std::fs::read_dir(&outside)
            .expect("outside remains readable")
            .next()
            .is_none(),
        "the descriptor-relative journal writer never followed the symlink"
    );
}
