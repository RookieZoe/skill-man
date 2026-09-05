use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::Connection;
use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreviewOutcome, SourceGroupPreviewService,
    SourceTrackingOverride,
};
use skill_man_lib::core::source_transition::{
    ConfirmSourceTransitionRequest, SourceTransitionError, SourceTransitionService,
};
use skill_man_lib::core::write_gate::WriteGate;
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::seams::installer_lock_store::{
    InstallerLockError, InstallerLockStore, LockFileFault, LockFileReport,
};
use skill_man_lib::seams::source::{GitFetchReport, GitSource, GitTreeEntry, SourceError};
use skill_man_lib::seams::source_transition_store::{
    ExistingSourceFacts, ExistingSourceMember, SourceTransitionRecord, SourceTransitionStore,
    SourceTransitionStoreError,
};

struct FixtureGitSource {
    inner: SystemGitSource,
    fixture_url: String,
}

#[derive(Default)]
struct EmptyLocks;

impl InstallerLockStore for EmptyLocks {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        Ok(Vec::new())
    }
}

struct FaultedLocks;

impl InstallerLockStore for FaultedLocks {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        Ok(vec![LockFileReport {
            path: PathBuf::from("/tmp/.skill-lock.json"),
            fingerprint: "faulted".into(),
            byte_len: 0,
            version: 4,
            fault: Some(LockFileFault::UnsupportedVersion(4)),
            entries: Vec::new(),
            entry_faults: Vec::new(),
        }])
    }
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

struct FailOnceSourceStore {
    inner: Arc<SqliteCatalogStore>,
    fail_commit_once: AtomicBool,
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
    fn existing_current_members(
        &self,
        canonical_url: &str,
    ) -> Result<Option<Vec<ExistingSourceMember>>, SourceTransitionStoreError> {
        self.inner.existing_current_members(canonical_url)
    }

    fn existing_source(
        &self,
        remote_id: &str,
    ) -> Result<Option<ExistingSourceFacts>, SourceTransitionStoreError> {
        self.inner.existing_source(remote_id)
    }

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
    write_gate: Arc<WriteGate>,
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
    let write_gate = Arc::new(WriteGate::open_for_tests());
    Fixture {
        _workspace: workspace,
        home,
        library,
        preview,
        source,
        locks,
        filesystem,
        catalog,
        write_gate,
    }
}

fn confirmation(fixture: &Fixture) -> ConfirmSourceTransitionRequest {
    let outcome = fixture
        .preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("expected clean preview");
    };
    assert_eq!(preview.members.len(), 2);
    ConfirmSourceTransitionRequest {
        source_type: "git".into(),
        source_url: preview.source_url,
        tracking_policy: Some(SourceTrackingOverride {
            mode: "branch".into(),
            value: Some("main".into()),
        }),
        expected_selected_ref: preview.policy.selected_ref,
        expected_resolved_commit: preview.policy.resolved_commit,
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
        fixture.write_gate.clone(),
    )
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

fn namespace_members(fixture: &Fixture, remote_id: &str) -> PathBuf {
    fixture.library.join("skills/git").join(remote_id)
}

#[test]
fn confirms_and_undoes_the_complete_source_in_one_release() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("v9 transition");

    assert_eq!(result.member_count, 2);
    assert!(result.undo_available);
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "remote_source_parents"), 1);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "git_source_releases"), 1);
    assert_eq!(count(&connection, "git_source_release_members"), 2);
    assert_eq!(count(&connection, "git_source_members"), 2);
    assert_eq!(count(&connection, "skills"), 2);
    let source: (String, String, String, String) = connection
        .query_row(
            "SELECT tracking_mode, tracking_value, current_selected_ref, current_release_id
             FROM git_repository_sources",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("source row");
    assert_eq!(source.0, "branch");
    assert_eq!(source.1, "main");
    assert_eq!(source.2, "main");
    assert!(source.3.starts_with("source-release-"));
    let selection_kind: String = connection
        .query_row(
            "SELECT selection_kind FROM git_source_releases",
            [],
            |row| row.get(0),
        )
        .expect("release row");
    assert_eq!(selection_kind, "branch");
    let storage: Vec<String> = connection
        .prepare("SELECT storage_relpath FROM git_source_members ORDER BY skill_path")
        .expect("members")
        .query_map([], |row| row.get(0))
        .expect("map")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect");
    assert_eq!(storage.len(), 2);
    assert!(
        storage
            .iter()
            .all(|path| { path.starts_with(&format!("skills/git/{}/", result.remote_id)) })
    );
    drop(connection);

    // Immutable namespace snapshots exist under skills/git/<remote_id>/<skill_id>.
    let namespace = namespace_members(&fixture, &result.remote_id);
    let member_ids: Vec<String> = open_catalog(&fixture)
        .prepare("SELECT skill_id FROM git_source_members ORDER BY skill_path")
        .expect("ids")
        .query_map([], |row| row.get(0))
        .expect("map")
        .collect::<Result<Vec<_>, _>>()
        .expect("ids");
    for skill_id in member_ids {
        assert!(
            namespace.join(&skill_id).is_dir(),
            "namespace snapshot for {skill_id} must be published"
        );
    }
    // The single full-file CAS released both claims.
    assert!(
        fixture.locks.discover().expect("read released lock")[0]
            .entries
            .is_empty(),
        "one CAS released both claims"
    );
    // The manifest freezes policy + selected ref + release.
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .library
                .join("remotes")
                .join(&result.remote_id)
                .join("source.json"),
        )
        .expect("source manifest"),
    )
    .expect("parse manifest");
    assert_eq!(manifest["trackingMode"], "branch");
    assert_eq!(manifest["trackingValue"], "main");
    assert_eq!(manifest["currentSelectedRef"], "main");
    assert_eq!(manifest["currentReleaseId"], result.release_id.clone());

    // Source Undo restores the whole source, the lock and the external
    // entities without touching other Home state.
    let undo = transition
        .undo(&result.operation_id)
        .expect("v9 source undo");
    assert_eq!(undo.member_count, 2);
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "remote_source_parents"), 0);
    assert_eq!(count(&connection, "git_repository_sources"), 0);
    assert_eq!(count(&connection, "git_source_releases"), 0);
    assert_eq!(count(&connection, "skills"), 0);
    drop(connection);
    assert!(
        !namespace_members(&fixture, &result.remote_id).exists(),
        "the namespace tree is removed on Undo"
    );
    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
    assert!(fixture.home.join(".agents/skills/beta/SKILL.md").is_file());
    assert_eq!(
        fixture.locks.discover().expect("read restored lock")[0]
            .entries
            .len(),
        2,
        "Undo restores the exact claim set"
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
fn confirms_a_worktree_only_source_without_fabricating_ownership_claims() {
    let fixture = fixture();
    let locks: Arc<dyn InstallerLockStore> = Arc::new(EmptyLocks);
    let preview = Arc::new(SourceGroupPreviewService::new(
        fixture.source.clone(),
        locks.clone(),
    ));
    let transition = SourceTransitionService::new(
        preview.clone(),
        fixture.source.clone(),
        locks,
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );
    let outcome = preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("expected a worktree-only preview");
    };
    assert!(preview.external_ownership_claims.is_empty());
    let result = transition
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
        .expect("unowned source converts into a managed release");
    assert_eq!(result.member_count, 2);
    assert!(
        fixture
            .filesystem
            .read_remote_parent_manifest(&fixture.library.join("remotes"), &result.remote_id)
            .expect("read source manifest")
            .is_some()
    );
    assert_eq!(count(&open_catalog(&fixture), "git_repository_sources"), 1);
    transition
        .undo(&result.operation_id)
        .expect("unowned source Undo");
    assert_eq!(count(&open_catalog(&fixture), "git_repository_sources"), 0);
}

#[test]
fn refuses_lockless_conversion_when_an_installer_lock_is_faulted() {
    let fixture = fixture();
    let locks: Arc<dyn InstallerLockStore> = Arc::new(FaultedLocks);
    let preview = Arc::new(SourceGroupPreviewService::new(
        fixture.source.clone(),
        locks.clone(),
    ));
    let transition = SourceTransitionService::new(
        preview.clone(),
        fixture.source.clone(),
        locks,
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );
    let outcome = preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("the faulted lock is hidden from the read-only preview");
    };
    let error = transition
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
        .expect_err("a faulted lock cannot be treated as no ownership");
    assert!(
        matches!(&error, SourceTransitionError::Validation(message) if message.contains("lock")),
        "expected a closed lock validation, got {error:?}"
    );
    assert_eq!(count(&open_catalog(&fixture), "git_repository_sources"), 0);
}

#[test]
fn second_confirmation_is_closed_to_update_until_ticket_93() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect("first transition");
    let error = transition
        .confirm(confirmation(&fixture))
        .expect_err("a managed source never re-enters Fetch Latest and Manage");
    let matches_closed = matches!(
        &error,
        SourceTransitionError::Validation(message) if message.contains("already managed")
    );
    assert!(
        matches_closed,
        "a whole-source Update belongs to ticket #93: {error}"
    );
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "skills"), 2);
    drop(connection);
    assert_eq!(
        fixture.locks.discover().expect("lock")[0].entries.len(),
        0,
        "the managed source's claims stay released"
    );
}

#[test]
fn journal_freezes_policy_manifest_and_namespace_before_the_commit_point() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect("transition");
    let journal = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .expect("frozen journal")
        .into_iter()
        .next()
        .expect("one journal");
    assert_eq!(journal.version, 2);
    assert_eq!(journal.tracking_mode, "branch");
    assert_eq!(journal.tracking_value.as_deref(), Some("main"));
    assert_eq!(journal.selection_kind, "branch");
    assert_eq!(journal.selected_ref, "main");
    assert_eq!(journal.members.len(), 2);
    for member in &journal.members {
        assert!(member.tree_hash.starts_with("tree-sha256-v1:"));
        assert!(
            member
                .namespace_path
                .starts_with(fixture.library.join("skills/git").join(&journal.remote_id))
        );
        assert_eq!(
            member.namespace_path.to_string_lossy(),
            format!(
                "{}/skills/git/{}/{}",
                fixture.library.to_string_lossy(),
                journal.remote_id,
                member.skill_id
            )
        );
    }
}

#[test]
fn confirmation_refuses_a_home_destination_before_releasing_external_ownership() {
    let fixture = fixture();
    // A plain file at the namespace root makes every member destination
    // unresolvable before the ownership CAS.
    std::fs::create_dir_all(fixture.library.join("skills")).expect("skills root");
    std::fs::write(fixture.library.join("skills/git"), "occupied").expect("occupant file");
    let transition = service(&fixture, fixture.catalog.clone());

    let error = transition
        .confirm(confirmation(&fixture))
        .expect_err("a Home destination collision is pre-CAS stale evidence");

    assert!(
        matches!(
            error,
            SourceTransitionError::Source(_)
                | SourceTransitionError::Validation(_)
                | SourceTransitionError::FileSystem(_)
        ),
        "unexpected error: {error}"
    );
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 0);
    assert_eq!(count(&connection, "skills"), 0);
    drop(connection);
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
            .is_empty(),
        "journal remains after a pre-CAS refusal: {error}"
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
    assert_eq!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("pending journal")
            .len(),
        1
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
        fixture.write_gate.clone(),
    );
    recovery
        .recover_pending(&fixture.library)
        .expect("roll forward the frozen journal");
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "git_source_releases"), 1);
    assert_eq!(count(&connection, "skills"), 2);
    drop(connection);
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("finished journal")
            .is_empty()
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
        fixture.write_gate.clone(),
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
        fixture.write_gate.clone(),
    );

    let error = recovery
        .recover_pending(&fixture.library)
        .expect_err("a same-key claim replacement is not the frozen pre-CAS state");

    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
}

#[test]
fn undo_rechecks_external_occupancy_before_mutating_the_catalog() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("transition");
    // A concurrent external owner reappears before Undo.
    std::fs::create_dir_all(fixture.home.join(".agents/skills/source")).expect("reappear");
    std::fs::write(
        fixture.home.join(".agents/skills/source/SKILL.md"),
        "---\nname: Source Root\n---\n# external\n",
    )
    .expect("external bytes");

    let error = transition
        .undo(&result.operation_id)
        .expect_err("an occupied external location blocks the whole Undo");

    assert!(matches!(error, SourceTransitionError::Validation(_)));
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "skills"), 2);
    drop(connection);
}

#[test]
fn recovery_refuses_a_journal_isolation_path_outside_the_derived_source_root() {
    let fixture = fixture();
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let transition = service(&fixture, flaky.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect_err("leave a post-CAS journal for recovery validation");
    let journal = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .expect("frozen journal")
        .into_iter()
        .next()
        .expect("one frozen journal");
    let journal_path = fixture
        .library
        .join("operations")
        .join(&journal.operation_id)
        .join("source-transition-journal.json");
    let forged = fixture
        .library
        .parent()
        .expect("workspace")
        .join(".skill-man-source-transition-forged");
    std::fs::create_dir_all(&forged).expect("forged isolation directory");
    std::fs::write(forged.join("keep"), "must not be deleted").expect("forged content");
    let mut journal_value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal_path).expect("read journal"))
            .expect("journal JSON");
    journal_value["members"][0]["isolated_path"] =
        serde_json::Value::String(forged.to_string_lossy().into_owned());
    std::fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&journal_value).expect("rewrite forged journal"),
    )
    .expect("write forged journal");
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
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

#[test]
fn repository_rename_and_path_aliases_never_move_the_storage_path() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("transition");
    // The storage path is keyed by remote_id + skill_id only: neither the
    // canonical URL nor a future alias affects it.
    let expected: Vec<String> = open_catalog(&fixture)
        .prepare("SELECT storage_relpath FROM git_source_members ORDER BY skill_path")
        .expect("members")
        .query_map([], |row| row.get(0))
        .expect("map")
        .collect::<Result<Vec<_>, _>>()
        .expect("paths");
    assert_eq!(expected.len(), 2);
    assert!(
        expected
            .iter()
            .all(|path| path.starts_with(&format!("skills/git/{}/", result.remote_id))),
        "every storage path is keyed by the stable remote_id"
    );
    assert!(
        expected.iter().all(|path| path.matches('/').count() == 3),
        "storage paths are exactly skills/git/<remote_id>/<skill_id>"
    );
}
