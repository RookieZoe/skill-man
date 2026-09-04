use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use rusqlite::Connection;
use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::domain::skill_identity_key;
use skill_man_lib::core::source_group_preview::{
    SourceGroupPreviewService, SourceTrackingOverride,
};
use skill_man_lib::core::source_promotion::{
    ConfirmSourcePromotionRequest, SourcePromotionDraftOutcome, SourcePromotionService,
};
use skill_man_lib::core::source_transition::SourceTransitionService;
use skill_man_lib::core::write_gate::WriteGate;
use skill_man_lib::seams::clock::Clock;

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
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::seams::installer_lock_store::InstallerLockStore;
use skill_man_lib::seams::source::{GitFetchReport, GitSource, GitTreeEntry, SourceError};

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

/// Target release: alpha kept at a new path, gamma added, beta disappears.
fn fixture_repository(root: &Path) -> PathBuf {
    let repository = root.join("source-repository");
    std::fs::create_dir_all(&repository).expect("repository");
    write_file(
        &repository,
        "skills/alpha/SKILL.md",
        "---\nname: Alpha\ndescription: Kept legacy member\n---\n# Alpha\n",
    );
    write_file(
        &repository,
        "skills/gamma/SKILL.md",
        "---\nname: Gamma\ndescription: A new member\n---\n# Gamma\n",
    );
    git(&repository, &["init", "-q", "-b", "main"]);
    git(
        &repository,
        &["config", "user.email", "fixture@example.com"],
    );
    git(&repository, &["config", "user.name", "Fixture"]);
    git(&repository, &["add", "-A"]);
    git(&repository, &["commit", "-q", "-m", "release"]);
    repository
}

struct Fixture {
    _workspace: tempfile::TempDir,
    home: PathBuf,
    library: PathBuf,
    source: Arc<dyn GitSource>,
    locks: Arc<SystemInstallerLockStore>,
    filesystem: Arc<MacOsFileSystem>,
    transition: Arc<SourceTransitionService>,
    service: Arc<SourcePromotionService>,
}

fn fixture() -> Fixture {
    let workspace = tempfile::tempdir().expect("workspace");
    let repository = fixture_repository(workspace.path());
    let home = workspace.path().join("home");
    let library = workspace.path().join("library");
    std::fs::create_dir_all(&library).expect("Library");
    // Legacy per-Skill state: alpha + beta under the external Skills root.
    write_file(
        &home.join(".agents/skills/alpha"),
        "SKILL.md",
        "---\nname: Alpha\ndescription: Kept legacy member\n---\n# Alpha\n",
    );
    write_file(
        &home.join(".agents/skills/beta"),
        "SKILL.md",
        "---\nname: Beta\ndescription: Gone upstream\n---\n# Beta\n",
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

    let catalog =
        Arc::new(SqliteCatalogStore::open(&library.join("skill-man.sqlite3")).expect("Catalog"));
    // Legacy parent + per-Skill skills/bindings (v7 state).
    connection(&library)
        .execute(
            "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
             VALUES ('remote-legacy', 'https://example.com/acme/source', 'then')",
            [],
        )
        .expect("legacy parent");
    for (id, name) in [("legacy-alpha", "alpha"), ("legacy-beta", "beta")] {
        connection(&library)
            .execute(
                "INSERT INTO skills (
                    id, directory_name, directory_identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path,
                    recorded_content_hash, health, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?2, '', 'remote_install', ?4, ?4,
                    'tree-sha256-v1:legacy', 'healthy', 'now', 'now')",
                rusqlite::params![
                    id,
                    name,
                    skill_identity_key(name),
                    home.join(".agents/skills").join(name).to_string_lossy(),
                ],
            )
            .expect("legacy skill");
        connection(&library)
            .execute(
                "INSERT INTO remote_bindings (
                    skill_id, remote_id, requested_ref, verification_anchor_commit,
                    original_commit_known, skill_path, provider_hash,
                    remote_baseline_hash, current_baseline_hash
                 ) VALUES (?1, 'remote-legacy', 'main', 'old-anchor', 1, ?2, NULL,
                    'tree-sha256-v1:legacy', 'tree-sha256-v1:legacy')",
                rusqlite::params![id, format!("skills/{name}")],
            )
            .expect("legacy binding");
    }

    let source: Arc<dyn GitSource> = Arc::new(FixtureGitSource::new(&repository));
    let locks = Arc::new(SystemInstallerLockStore::new(home.clone()));
    let preview = Arc::new(SourceGroupPreviewService::new(
        source.clone(),
        locks.clone(),
    ));
    let filesystem = Arc::new(MacOsFileSystem::new(home.clone()));
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
            write_gate,
        )
        .with_promotion_store(catalog.clone()),
    );
    let service = Arc::new(SourcePromotionService::new(
        preview,
        catalog.clone(),
        transition.clone(),
    ));
    Fixture {
        _workspace: workspace,
        home,
        library,
        source,
        locks,
        filesystem,
        transition,
        service,
    }
}

fn connection(library: &Path) -> Connection {
    Connection::open(library.join("skill-man.sqlite3")).expect("Catalog connection")
}

fn count(library: &Path, table: &str) -> i64 {
    connection(library)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or_else(|error| panic!("count rows in {table}: {error}"))
}

fn promotion_request(remote_id: &str) -> ConfirmSourcePromotionRequest {
    let canonical_url = "https://example.com/acme/source";
    ConfirmSourcePromotionRequest {
        remote_id: remote_id.into(),
        source_type: "git".into(),
        source_url: canonical_url.into(),
        tracking_policy: Some(SourceTrackingOverride {
            mode: "branch".into(),
            value: Some("main".into()),
        }),
        expected_selected_ref: "main".into(),
        expected_resolved_commit: "0000000000000000000000000000000000000000".into(),
    }
}

#[test]
fn legacy_source_promotion_uses_one_transition_and_undoes_whole() {
    let fixture = fixture();
    // The read-only preview classifies the complete release against the
    // legacy members: alpha is current, gamma is added, beta is removed.
    let outcome = fixture
        .service
        .preview(
            "remote-legacy",
            Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        )
        .expect("promotion preview");
    let SourcePromotionDraftOutcome::Draft(draft) = outcome else {
        panic!("an unambiguous legacy parent must draft");
    };
    let paths = draft
        .members
        .iter()
        .map(|member| member.skill_path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 2);
    assert!(draft.members.iter().any(|member| {
        member.skill_path == "skills/alpha"
            && member.state
                == skill_man_lib::core::source_promotion::SourcePromotionMemberState::Current
    }));
    assert!(draft.members.iter().any(|member| {
        member.skill_path == "skills/gamma"
            && member.state
                == skill_man_lib::core::source_promotion::SourcePromotionMemberState::Added
    }));
    assert_eq!(draft.removed_members.len(), 1);
    assert_eq!(draft.removed_members[0].skill_path, "skills/beta");
    let request = promotion_request(&draft.remote_id);
    let request = ConfirmSourcePromotionRequest {
        expected_resolved_commit: draft.policy.resolved_commit.clone(),
        expected_selected_ref: draft.policy.selected_ref.clone(),
        ..request
    };

    // Confirmation runs the same transition journal/CAS as a clean source:
    // single prompt-free re-discovery, isolation, one full-file CAS and one
    // whole-source catalog commit.
    let result = fixture
        .service
        .confirm(request)
        .expect("whole-source promotion");
    assert_eq!(result.member_count, 2);
    assert_eq!(
        result.remote_id, "remote-legacy",
        "the parent id is retained"
    );
    assert_eq!(count(&fixture.library, "git_repository_sources"), 1);
    assert_eq!(count(&fixture.library, "git_source_releases"), 1);
    assert_eq!(count(&fixture.library, "git_source_members"), 2);
    assert_eq!(count(&fixture.library, "skills"), 2);
    assert_eq!(
        count(&fixture.library, "remote_bindings"),
        0,
        "bindings are audit-only"
    );
    let beta_gone: bool = connection(&fixture.library)
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM skills WHERE id = 'legacy-beta')",
            [],
            |row| row.get(0),
        )
        .expect("removed member state");
    assert!(!beta_gone);
    let alpha_path: String = connection(&fixture.library)
        .query_row(
            "SELECT final_entity_path FROM skills WHERE id = 'legacy-alpha'",
            [],
            |row| row.get(0),
        )
        .expect("converted member");
    assert_eq!(
        alpha_path,
        "skills/git/remote-legacy/legacy-alpha".to_string(),
        "the legacy member moves into the immutable namespace"
    );
    // The immutable snapshot is published and the single CAS released both
    // external claims.
    assert!(
        fixture
            .library
            .join("skills/git/remote-legacy/legacy-alpha")
            .is_dir()
    );
    assert!(
        fixture
            .library
            .join("skills/git/remote-legacy")
            .join(
                connection(&fixture.library)
                    .query_row::<String, _, _>(
                        "SELECT skill_id FROM git_source_members WHERE skill_path = 'skills/gamma'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("gamma id"),
            )
            .is_dir()
    );
    assert!(
        fixture.locks.discover().expect("lock")[0]
            .entries
            .is_empty(),
        "one CAS released the whole legacy claim set"
    );

    // Source Undo restores the frozen whole Legacy source in one guard-checked
    // action; the legacy entity bytes and the exact claims come back.
    let undo = fixture
        .transition
        .undo(&result.operation_id)
        .expect("promotion undo");
    assert_eq!(undo.member_count, 2);
    assert_eq!(count(&fixture.library, "git_repository_sources"), 0);
    assert_eq!(count(&fixture.library, "skills"), 2);
    assert_eq!(count(&fixture.library, "remote_bindings"), 2);
    let beta_back: bool = connection(&fixture.library)
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM skills WHERE id = 'legacy-beta')",
            [],
            |row| row.get(0),
        )
        .expect("restored member");
    assert!(beta_back);
    assert!(fixture.home.join(".agents/skills/alpha/SKILL.md").is_file());
    assert!(fixture.home.join(".agents/skills/beta/SKILL.md").is_file());
    assert_eq!(
        fixture.locks.discover().expect("restored lock")[0]
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
fn promotion_preview_refuses_an_ambiguous_or_missing_legacy_parent() {
    let fixture = fixture();
    let error = fixture
        .service
        .preview("remote-missing", None)
        .expect_err("no legacy parent");
    assert!(error.to_string().contains("parent"));
    let _ = &fixture.source;
}

#[test]
fn promotion_keeps_the_policy_selection_in_the_manifest() {
    let fixture = fixture();
    let outcome = fixture
        .service
        .preview("remote-legacy", None)
        .expect("default policy preview");
    let SourcePromotionDraftOutcome::Draft(draft) = outcome else {
        panic!("draft");
    };
    // The fixture has no provider releases, tags or reachable versions, so
    // the default policy falls through to HEAD deterministically.
    assert_eq!(draft.policy.mode, "auto_release_tag_head");
    assert_eq!(draft.policy.selection_kind, "head");
    assert_eq!(draft.policy.selected_ref, "HEAD");
}
