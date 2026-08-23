use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreviewOutcome, SourceGroupPreviewService,
};
use skill_man_lib::seams::installer_lock_store::{
    InstallerLockError, InstallerLockStore, LockEntry, LockFileReport,
};
use skill_man_lib::seams::source::{GitFetchReport, GitSource, GitTreeEntry, SourceError};

#[derive(Default)]
struct EmptyLocks;

impl InstallerLockStore for EmptyLocks {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        Ok(Vec::new())
    }
}

struct StaticLocks(Vec<LockFileReport>);

impl InstallerLockStore for StaticLocks {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        Ok(self.0.clone())
    }
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

fn lock_report(path: &Path, ref_name: &str) -> LockFileReport {
    LockFileReport {
        path: path.to_path_buf(),
        fingerprint: "a".repeat(64),
        byte_len: 0,
        version: 3,
        fault: None,
        entries: vec![LockEntry {
            name: "legacy-skill".into(),
            source_type: "git".into(),
            source: "legacy installer".into(),
            source_url: "https://example.com/acme/source.git".into(),
            requested_ref: Some(ref_name.into()),
            skill_path: "legacy-skill".into(),
            skill_folder_hash: "0".repeat(64),
            installed_at: None,
            updated_at: None,
            plugin_name: None,
        }],
        entry_faults: Vec::new(),
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_file(repo: &Path, path: &str, contents: &str) {
    let target = repo.join(path);
    std::fs::create_dir_all(target.parent().expect("file parent")).expect("create parent");
    std::fs::write(target, contents).expect("write fixture");
}

fn committed_repo(root: &Path) -> PathBuf {
    let repo = root.join("source-repo");
    std::fs::create_dir_all(&repo).expect("create repository");
    write_file(&repo, "SKILL.md", "---\nname: Root Skill\n---\n# Root\n");
    write_file(
        &repo,
        "packages/beta/SKILL.md",
        "---\nname: Beta Skill\ndescription: Full-source member\n---\n# Beta\n",
    );
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "fixture@example.com"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "source release"]);
    repo
}

#[test]
fn fetch_latest_and_manage_previews_every_discovered_member_without_writing_a_draft() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let repo = committed_repo(workspace.path());
    let home = workspace.path().join("home");
    std::fs::create_dir_all(&home).expect("create Home");
    let lock_path = home.join(".skill-lock.json");
    std::fs::write(&lock_path, "{\"version\":3,\"skills\":{}}\n").expect("write lock");
    let lock_before = std::fs::read(&lock_path).expect("read lock before preview");

    let service = SourceGroupPreviewService::new(
        Arc::new(FixtureGitSource::new(&repo)),
        Arc::new(EmptyLocks),
    );
    let outcome = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source/".into(),
            tracking_ref: Some("main".into()),
        })
        .expect("read-only source preview");

    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("a single unambiguous source should preview");
    };
    assert_eq!(preview.provider, "git");
    assert_eq!(preview.tracking_ref, "main");
    assert_eq!(preview.members.len(), 2);
    assert_eq!(
        preview
            .members
            .iter()
            .map(|member| member.skill_path.as_str())
            .collect::<Vec<_>>(),
        vec!["", "packages/beta"]
    );
    assert!(
        preview
            .members
            .iter()
            .all(|member| member.tree_summary.len() == 40)
    );
    assert!(preview.external_ownership_claims.is_empty());
    assert_eq!(
        std::fs::read(&lock_path).expect("read lock after preview"),
        lock_before,
        "preview must not rewrite an external lock"
    );
    assert!(
        !home.join("skill-man.sqlite3").exists()
            && !home.join("cache").exists()
            && !home.join("staging").exists()
            && !home.join("operations").exists(),
        "preview must not create Home, staging, or journal state"
    );
}

#[test]
fn legacy_ref_conflict_requires_selection_before_fetching_or_writing() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let service = SourceGroupPreviewService::new(
        Arc::new(SystemGitSource::new()),
        Arc::new(StaticLocks(vec![
            lock_report(&workspace.path().join("legacy/.skill-lock.json"), "main"),
            lock_report(&workspace.path().join("other/.skill-lock.json"), "release"),
        ])),
    );

    let outcome = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source/".into(),
            tracking_ref: None,
        })
        .expect("conflict is a typed read-only result");

    let SourceGroupPreviewOutcome::RepositoryRefConflict(conflict) = outcome else {
        panic!("unselected conflicting legacy refs must not fetch a preview");
    };
    assert_eq!(conflict.available_refs, vec!["main", "release"]);
    assert_eq!(conflict.external_ownership_claims.len(), 2);
    assert!(
        !workspace.path().join("home").exists(),
        "ref conflict must resolve before a persistent draft"
    );
}

#[test]
fn ownership_split_refuses_confirmation_without_fetching_or_writing() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let service = SourceGroupPreviewService::new(
        Arc::new(SystemGitSource::new()),
        Arc::new(StaticLocks(vec![
            lock_report(&workspace.path().join("first/.skill-lock.json"), "main"),
            lock_report(&workspace.path().join("second/.skill-lock.json"), "main"),
        ])),
    );

    let outcome = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source.git".into(),
            tracking_ref: Some("main".into()),
        })
        .expect("split is a typed read-only result");

    let SourceGroupPreviewOutcome::RepositoryOwnershipSplit(split) = outcome else {
        panic!("ownership split must refuse confirmation");
    };
    assert_eq!(split.tracking_ref, "main");
    assert_eq!(split.lock_paths.len(), 2);
    assert!(
        !workspace.path().join("home").exists(),
        "ownership split must not create a persistent draft"
    );
}

#[test]
fn generic_source_requires_https_before_creating_a_cache_entry() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let service =
        SourceGroupPreviewService::new(Arc::new(SystemGitSource::new()), Arc::new(EmptyLocks));

    let error = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "http://example.com/acme/source.git".into(),
            tracking_ref: Some("main".into()),
        })
        .expect_err("generic non-HTTPS sources are not supported");

    assert!(error.to_string().contains("HTTPS"));
    assert!(
        !workspace.path().join("home").exists(),
        "validation must happen before a persistent preview artifact"
    );
}

#[test]
fn symlinked_skill_documents_refuse_a_partial_source_release() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let repo = committed_repo(workspace.path());
    let unsafe_directory = repo.join("packages/unsafe");
    std::fs::create_dir_all(&unsafe_directory).expect("create unsafe fixture directory");
    std::os::unix::fs::symlink("../../SKILL.md", unsafe_directory.join("SKILL.md"))
        .expect("create symlinked skill document");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "unsafe member"]);
    let home = workspace.path().join("home");
    std::fs::create_dir_all(&home).expect("create Home");

    let service = SourceGroupPreviewService::new(
        Arc::new(FixtureGitSource::new(&repo)),
        Arc::new(EmptyLocks),
    );
    let error = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_ref: Some("main".into()),
        })
        .expect_err("an unsafe member must not be silently omitted");

    assert!(error.to_string().contains("symlinked SKILL.md"));
    assert!(!home.join("cache").exists(), "no Home cache may be created");
}
