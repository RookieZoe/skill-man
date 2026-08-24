use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rusqlite::Connection;
use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::domain::SkillId;
use skill_man_lib::core::maintenance::MaintenanceService;
use skill_man_lib::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreviewOutcome, SourceGroupPreviewService,
};
use skill_man_lib::core::source_promotion::SourcePromotionService;
use skill_man_lib::core::source_transition::{
    ConfirmSourceTransitionRequest, SourceTransitionError, SourceTransitionService,
};
use skill_man_lib::core::source_update::{
    ConfirmSourceUpdateRequest, ModifiedMemberResolution, SourceUpdateError,
    SourceUpdateMemberState, SourceUpdateResolution, SourceUpdateService,
    UpstreamMemberRemovedResolution,
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
use skill_man_lib::seams::source_update_store::SourceUpdateStoreError;

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

/// Simulates an external installer writing a same-source declaration after
/// the final staged member is available but before any source mutation.
struct ReappearAfterStageGitSource {
    inner: Arc<dyn GitSource>,
    lock_path: PathBuf,
    staged_once: AtomicBool,
}

impl GitSource for ReappearAfterStageGitSource {
    fn fetch_mirror(&self, url: &str, mirror_dir: &Path) -> Result<GitFetchReport, SourceError> {
        self.inner.fetch_mirror(url, mirror_dir)
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
            .stage_skill(mirror_dir, commit, skill_path, destination)?;
        if !self.staged_once.swap(true, Ordering::SeqCst) {
            std::fs::write(
                &self.lock_path,
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
    }
  }
}"#,
            )
            .map_err(|source| SourceError::Io {
                operation: "reappear external installer after staging",
                path: self.lock_path.clone(),
                source,
            })?;
        }
        Ok(())
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

#[derive(Default)]
struct AdvancingClock {
    ticks: AtomicUsize,
}

impl Clock for AdvancingClock {
    fn monotonic_millis(&self) -> u128 {
        self.ticks.fetch_add(1, Ordering::SeqCst) as u128
    }

    fn unix_epoch_nanos(&self) -> u128 {
        1_725_000_000_000_000_000 + self.ticks.fetch_add(1, Ordering::SeqCst) as u128
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

/// Reintroduces the same external declaration only after an Update has
/// produced its complete draft. The confirmation guard must see it before
/// any source mutation.
struct ReappearOnPrepareLockStore {
    inner: Arc<SystemInstallerLockStore>,
    path: PathBuf,
    discover_calls: AtomicUsize,
}

impl InstallerLockStore for ReappearOnPrepareLockStore {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        if self.discover_calls.fetch_add(1, Ordering::SeqCst) == 2 {
            std::fs::write(
                &self.path,
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
    }
  }
}"#,
            )
            .expect("external installer reappears during Update confirmation");
        }
        self.inner.discover()
    }
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
fn source_update_rejects_a_release_that_changed_after_the_complete_draft() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");

    let source_root = fixture._workspace.path().join("source-repository");
    write_file(
        &source_root,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: First update\n---\n# Beta\n",
    );
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "first update",
        ],
    );

    let remote_id: String = Connection::open(fixture.library.join("skill-man.sqlite3"))
        .expect("Catalog connection")
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());
    let draft = update
        .preview(&remote_id)
        .expect("complete Source Update draft");
    assert_eq!(
        draft.target_members.len(),
        2,
        "the source remains all-members"
    );

    write_file(
        &source_root,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: Later update\n---\n# Beta\n",
    );
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "later update",
        ],
    );

    let error = update
        .confirm(ConfirmSourceUpdateRequest {
            remote_id: remote_id.clone(),
            expected_resolved_commit: draft.resolved_commit,
            resolutions: Vec::new(),
        })
        .expect_err("a changed remote makes the complete Source Group Draft stale");
    assert!(matches!(error, SourceUpdateError::PlanStale));
}

#[test]
fn source_update_advances_every_member_to_one_new_complete_release() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    let source_root = fixture._workspace.path().join("source-repository");
    write_file(
        &source_root,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: Updated as one source\n---\n# Beta\n",
    );
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "source update",
        ],
    );
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());
    let draft = update
        .preview(&remote_id)
        .expect("complete Source Update draft");
    let result = update
        .confirm(ConfirmSourceUpdateRequest {
            remote_id: remote_id.clone(),
            expected_resolved_commit: draft.resolved_commit,
            resolutions: Vec::new(),
        })
        .expect("commit one complete Source Update");

    assert!(
        !fixture
            .filesystem
            .list_source_promotion_journals(&fixture.library)
            .expect("confirm retains the non-undoable result until restart recovery")
            .is_empty()
    );

    assert_eq!(result.member_count, 2);
    assert!(
        std::fs::read_to_string(fixture.library.join("skills/beta/SKILL.md"))
            .expect("updated beta")
            .contains("Updated as one source")
    );
    let release_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_source_releases", [], |row| {
            row.get(0)
        })
        .expect("prior and current releases");
    let current_members: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM git_source_members WHERE remote_id = ?1",
            [&remote_id],
            |row| row.get(0),
        )
        .expect("complete current source members");
    assert_eq!((release_count, current_members), (2, 2));
    let legacy_recovery = SourcePromotionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
    )
    .with_lock_store(fixture.locks.clone());
    legacy_recovery
        .recover_pending(&fixture.library)
        .expect("Legacy Source Promotion recovery ignores Source Update journals");
    update
        .recover_pending(&fixture.library)
        .expect("Source Update recovery settles its frozen journal without refetching");
    assert!(
        fixture
            .filesystem
            .list_source_promotion_journals(&fixture.library)
            .expect("Source Update recovery finalizes the non-undoable result")
            .is_empty()
    );
}

#[test]
fn source_update_requires_and_applies_modified_and_removed_member_resolutions() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    write_file(
        &fixture.library.join("skills/source"),
        "LOCAL-NOTE.md",
        "This member has a local change.\n",
    );
    let source_root = fixture._workspace.path().join("source-repository");
    git(&source_root, &["rm", "-r", "skills/beta"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "remove beta from the source release",
        ],
    );
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());
    let draft = update
        .preview(&remote_id)
        .expect("complete Source Update draft");
    let source_member = draft
        .existing_members
        .iter()
        .find(|member| member.member.directory_name == "source")
        .expect("current source member");
    assert_eq!(
        source_member.state,
        SourceUpdateMemberState::ModifiedMemberResolutionRequired
    );
    let beta_member = draft
        .existing_members
        .iter()
        .find(|member| member.member.directory_name == "beta")
        .expect("removed source member");
    assert_eq!(
        beta_member.state,
        SourceUpdateMemberState::UpstreamMemberRemoved
    );

    update
        .confirm(ConfirmSourceUpdateRequest {
            remote_id: remote_id.clone(),
            expected_resolved_commit: draft.resolved_commit,
            resolutions: vec![
                SourceUpdateResolution {
                    skill_id: source_member.member.skill_id.clone(),
                    modified: Some(ModifiedMemberResolution::KeepModified),
                    removed: None,
                },
                SourceUpdateResolution {
                    skill_id: beta_member.member.skill_id.clone(),
                    modified: None,
                    removed: Some(UpstreamMemberRemovedResolution::Remove),
                },
            ],
        })
        .expect("apply every explicit Source Update resolution");

    assert!(
        fixture
            .library
            .join("skills/source/LOCAL-NOTE.md")
            .is_file()
    );
    assert!(!fixture.library.join("skills/beta").exists());
    let members: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM git_source_members WHERE remote_id = ?1",
            [&remote_id],
            |row| row.get(0),
        )
        .expect("remaining complete source member");
    assert_eq!(members, 1);

    write_file(
        &source_root,
        "skills/source/SKILL.md",
        "---\nname: Source Root\ndescription: Later remote source update\n---\n# Root\n",
    );
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "later remote update after keep modified",
        ],
    );
    let later_draft = update
        .preview(&remote_id)
        .expect("later complete Source Update draft");
    assert!(later_draft.existing_members.iter().any(|member| {
        member.member.directory_name == "source"
            && member.state == SourceUpdateMemberState::ModifiedMemberResolutionRequired
    }));
}

#[test]
fn source_update_reports_external_reappearance_as_ownership_conflict() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
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
      "skillFolderHash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  }
}"#,
    )
    .expect("external installer reappears");
    let remote_id: String = Connection::open(fixture.library.join("skill-man.sqlite3"))
        .expect("Catalog connection")
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());
    assert!(matches!(
        update.preview(&remote_id),
        Err(SourceUpdateError::OwnershipConflict)
    ));
    assert!(fixture.library.join("skills/source/SKILL.md").is_file());
}

#[test]
fn source_update_fails_closed_for_a_faulted_installer_lock() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    std::fs::write(
        fixture.home.join(".agents/.skill-lock.json"),
        "{ this is not a valid lock file",
    )
    .expect("fault the external installer lock");
    let remote_id: String = Connection::open(fixture.library.join("skill-man.sqlite3"))
        .expect("Catalog connection")
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());

    assert!(matches!(
        update.preview(&remote_id),
        Err(SourceUpdateError::OwnershipConflict)
    ));
}

#[test]
fn source_update_refuses_an_external_entity_reappearance_without_a_lock() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    write_file(
        &fixture.home.join(".agents/skills/source"),
        "SKILL.md",
        "---\nname: External source\ndescription: Reappeared owner\n---\n# External\n",
    );
    let remote_id: String = Connection::open(fixture.library.join("skill-man.sqlite3"))
        .expect("Catalog connection")
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());

    assert!(matches!(
        update.preview(&remote_id),
        Err(SourceUpdateError::OwnershipConflict)
    ));
    assert!(fixture.library.join("skills/source/SKILL.md").is_file());
}

#[test]
fn ordinary_remove_never_restores_a_reappeared_external_owner() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    write_file(
        &fixture.home.join(".agents/skills/source"),
        "SKILL.md",
        "---\nname: External source\ndescription: Reappeared owner\n---\n# External\n",
    );
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let skill_id: String = connection
        .query_row(
            "SELECT id FROM skills WHERE directory_name = 'source'",
            [],
            |row| row.get(0),
        )
        .expect("managed source member id");
    let runtime = Arc::new(RuntimeCatalogStore::new(
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
    ));
    let maintenance = MaintenanceService::new(runtime, fixture.filesystem.clone())
        .with_library_root(fixture.library.clone());
    let plan = maintenance
        .plan_remove(&SkillId(skill_id))
        .expect("plan ordinary Remove");
    maintenance
        .apply_remove(&plan.plan_token)
        .expect("apply ordinary Remove");

    assert!(!fixture.library.join("skills/source").exists());
    assert!(
        std::fs::read_to_string(fixture.home.join(".agents/skills/source/SKILL.md"))
            .expect("reappeared external entity remains untouched")
            .contains("Reappeared owner")
    );
}

#[test]
fn source_update_refuses_a_manifest_that_is_not_the_current_source_release() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    let remote_id: String = Connection::open(fixture.library.join("skill-man.sqlite3"))
        .expect("Catalog connection")
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let mut manifest = fixture
        .filesystem
        .read_remote_parent_manifest(&fixture.library.join("remotes"), &remote_id)
        .expect("read source manifest")
        .expect("managed source manifest");
    manifest.current_release_id = Some("foreign-release".into());
    fixture
        .filesystem
        .write_remote_parent_manifest(&fixture.library.join("remotes"), &manifest)
        .expect("tamper source manifest");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());

    assert!(matches!(
        update.preview(&remote_id),
        Err(SourceUpdateError::Validation(_))
    ));
}

#[test]
fn source_update_backfills_a_manifest_for_a_preexisting_managed_source() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    fixture
        .filesystem
        .remove_remote_parent_manifest(&fixture.library.join("remotes"), &remote_id)
        .expect("simulate a #63 source without the later manifest");
    let source_root = fixture._workspace.path().join("source-repository");
    write_file(
        &source_root,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: Backfill manifest\n---\n# Beta\n",
    );
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "backfill managed source manifest",
        ],
    );
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());
    let draft = update
        .preview(&remote_id)
        .expect("manifest-less source remains previewable without a write");
    assert!(
        fixture
            .filesystem
            .read_remote_parent_manifest(&fixture.library.join("remotes"), &remote_id)
            .expect("read absent manifest")
            .is_none()
    );
    let result = update
        .confirm(ConfirmSourceUpdateRequest {
            remote_id: remote_id.clone(),
            expected_resolved_commit: draft.resolved_commit,
            resolutions: Vec::new(),
        })
        .expect("commit source update and backfill manifest");
    let manifest = fixture
        .filesystem
        .read_remote_parent_manifest(&fixture.library.join("remotes"), &remote_id)
        .expect("read backfilled manifest")
        .expect("backfilled manifest");
    assert_eq!(
        manifest.current_release_id.as_deref(),
        Some(result.release_id.as_str())
    );
}

#[test]
fn source_update_refuses_a_current_member_set_that_disagrees_with_its_release() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    connection
        .execute(
            "UPDATE git_source_members SET remote_baseline_hash = 'tampered' WHERE remote_id = ?1",
            [&remote_id],
        )
        .expect("tamper current member facts");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());

    assert!(matches!(
        update.preview(&remote_id),
        Err(SourceUpdateError::Store(SourceUpdateStoreError::Conflict(
            _
        )))
    ));
}

#[test]
fn source_update_rechecks_an_external_declaration_before_staging() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    let source_root = fixture._workspace.path().join("source-repository");
    write_file(
        &source_root,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: New remote release\n---\n# Beta\n",
    );
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "source update guarded by reappearance",
        ],
    );
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let reappearing = Arc::new(ReappearOnPrepareLockStore {
        inner: fixture.locks.clone(),
        path: fixture.home.join(".agents/.skill-lock.json"),
        discover_calls: AtomicUsize::new(0),
    });
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(reappearing);
    let draft = update
        .preview(&remote_id)
        .expect("complete Source Update draft");

    assert!(matches!(
        update.confirm(ConfirmSourceUpdateRequest {
            remote_id: remote_id.clone(),
            expected_resolved_commit: draft.resolved_commit,
            resolutions: Vec::new(),
        }),
        Err(SourceUpdateError::OwnershipConflict)
    ));
    let release_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_source_releases", [], |row| {
            row.get(0)
        })
        .expect("unchanged current release");
    assert_eq!(release_count, 1);
    assert!(
        std::fs::read_to_string(fixture.library.join("skills/beta/SKILL.md"))
            .expect("current library member")
            .contains("Nested member")
    );
}

#[test]
fn source_update_rechecks_an_external_declaration_after_staging() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    let source_root = fixture._workspace.path().join("source-repository");
    write_file(
        &source_root,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: Late external guard\n---\n# Beta\n",
    );
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "source update guarded after staging",
        ],
    );
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let after_stage_source: Arc<dyn GitSource> = Arc::new(ReappearAfterStageGitSource {
        inner: fixture.source.clone(),
        lock_path: fixture.home.join(".agents/.skill-lock.json"),
        staged_once: AtomicBool::new(false),
    });
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        after_stage_source,
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());
    let draft = update
        .preview(&remote_id)
        .expect("complete Source Update draft");

    assert!(matches!(
        update.confirm(ConfirmSourceUpdateRequest {
            remote_id: remote_id.clone(),
            expected_resolved_commit: draft.resolved_commit,
            resolutions: Vec::new(),
        }),
        Err(SourceUpdateError::OwnershipConflict)
    ));
    let release_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_source_releases", [], |row| {
            row.get(0)
        })
        .expect("unchanged current release");
    assert_eq!(release_count, 1);
}

#[test]
fn source_update_recovery_refuses_a_tampered_current_baseline() {
    let fixture = fixture();
    service(&fixture, fixture.catalog.clone())
        .confirm(confirmation(&fixture))
        .expect("establish the complete Git Repository Source");
    let source_root = fixture._workspace.path().join("source-repository");
    write_file(
        &source_root,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: Fixed release for recovery\n---\n# Beta\n",
    );
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "fixed source update for recovery",
        ],
    );
    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("current source id");
    let update = SourceUpdateService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
    )
    .with_lock_store(fixture.locks.clone());
    let draft = update.preview(&remote_id).expect("complete update draft");
    update
        .confirm(ConfirmSourceUpdateRequest {
            remote_id: remote_id.clone(),
            expected_resolved_commit: draft.resolved_commit,
            resolutions: Vec::new(),
        })
        .expect("commit fixed source update");
    connection
        .execute(
            "UPDATE git_source_members SET current_baseline_hash = 'tampered' WHERE remote_id = ?1",
            [&remote_id],
        )
        .expect("tamper committed current baseline");

    assert!(matches!(
        update.recover_pending(&fixture.library),
        Err(SourceUpdateError::RecoveryRequired(_))
    ));
}

#[test]
fn source_transition_freezes_the_manifest_before_verifying_it() {
    let fixture = fixture();
    let transition = SourceTransitionService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        fixture.locks.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(AdvancingClock::default()),
        fixture.library.clone(),
        fixture.home.clone(),
    );
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("a changing clock does not invalidate the frozen manifest");

    let connection =
        Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection");
    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("managed source id");
    let manifest = fixture
        .filesystem
        .read_remote_parent_manifest(&fixture.library.join("remotes"), &remote_id)
        .expect("read managed source manifest")
        .expect("managed source manifest");
    assert_eq!(
        manifest.current_release_id.as_deref(),
        Some(result.release_id.as_str())
    );
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
