use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreviewOutcome, SourceGroupPreviewService,
    SourceTrackingOverride,
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
    fn validate_home_cache(&self, home: &Path, mirror: &Path) -> Result<(), SourceError> {
        self.inner.validate_home_cache(home, mirror)
    }

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
fn app_home_cache_survives_preview_refreshes_and_rebuilds_after_cleanup() {
    use skill_man_lib::core::git_source::git_mirror_path;
    use skill_man_lib::core::home::BoundHome;
    use skill_man_lib::core::write_gate::{WriteGate, WriteGateState};
    let workspace = tempfile::tempdir().unwrap();
    let repo = committed_repo(workspace.path());
    let home = workspace.path().join("app-selected-home");
    std::fs::create_dir(&home).unwrap();
    let gate = Arc::new(WriteGate::new(WriteGateState::Open(BoundHome::test_value(
        "cache-home",
        home.clone(),
    ))));
    let service = SourceGroupPreviewService::new(
        Arc::new(FixtureGitSource::new(&repo)),
        Arc::new(EmptyLocks),
    )
    .with_home_cache(gate.clone());
    let request = || FetchLatestAndManageRequest {
        source_type: "git".into(),
        source_url: "https://example.com/acme/source".into(),
        tracking_policy: Some(SourceTrackingOverride {
            mode: "branch".into(),
            value: Some("main".into()),
        }),
    };
    let mirror = git_mirror_path(&home.join("cache"), "https://example.com/acme/source");
    service.fetch_latest_and_manage(request()).unwrap();
    assert!(mirror.join("HEAD").is_file());
    let config = std::fs::read(mirror.join("config")).unwrap();
    write_file(
        &repo,
        "packages/added/SKILL.md",
        "---\nname: Added\n---\n# Added\n",
    );
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "new member"]);
    let SourceGroupPreviewOutcome::Preview(refreshed) =
        service.fetch_latest_and_manage(request()).unwrap()
    else {
        panic!("preview")
    };
    assert_eq!(refreshed.members.len(), 3);
    assert_eq!(std::fs::read(mirror.join("config")).unwrap(), config);
    assert!(!home.join("skills").exists());
    assert!(!home.join("operations").exists());
    std::fs::remove_dir_all(&mirror).unwrap();
    service.fetch_latest_and_manage(request()).unwrap();
    assert!(mirror.join("HEAD").is_file());
    let next_home = workspace.path().join("next-app-home");
    std::fs::create_dir(&next_home).unwrap();
    gate.transition_to(WriteGateState::Open(BoundHome::test_value(
        "next-home",
        next_home.clone(),
    )))
    .unwrap();
    service.fetch_latest_and_manage(request()).unwrap();
    let next_mirror = git_mirror_path(&next_home.join("cache"), "https://example.com/acme/source");
    assert!(next_mirror.join("HEAD").is_file());
    std::fs::remove_dir_all(&next_home).unwrap();
    assert!(service.fetch_latest_and_manage(request()).is_err());
    assert!(
        !next_home.exists(),
        "a missing bound Home must not be recreated"
    );
    gate.mark_blocked();
    assert!(service.fetch_latest_and_manage(request()).is_err());
}

#[cfg(unix)]
#[test]
fn app_home_cache_refuses_a_symlinked_cache_directory() {
    use skill_man_lib::core::home::BoundHome;
    use skill_man_lib::core::write_gate::{WriteGate, WriteGateState};
    let workspace = tempfile::tempdir().unwrap();
    let repo = committed_repo(workspace.path());
    let home = workspace.path().join("selected-home");
    let outside = workspace.path().join("outside");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, home.join("cache")).unwrap();
    let gate = Arc::new(WriteGate::new(WriteGateState::Open(BoundHome::test_value(
        "cache-home",
        home,
    ))));
    let service = SourceGroupPreviewService::new(
        Arc::new(FixtureGitSource::new(&repo)),
        Arc::new(EmptyLocks),
    )
    .with_home_cache(gate);
    assert!(
        service
            .fetch_latest_and_manage(FetchLatestAndManageRequest {
                source_type: "git".into(),
                source_url: "https://example.com/acme/source".into(),
                tracking_policy: None
            })
            .is_err()
    );
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
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
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("read-only source preview");

    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("a single unambiguous source should preview");
    };
    assert_eq!(preview.provider, "git");
    assert_eq!(preview.policy.selected_ref, "main");
    assert_eq!(preview.policy.selection_kind, "branch");
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
            tracking_policy: None,
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
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("split is a typed read-only result");

    let SourceGroupPreviewOutcome::RepositoryOwnershipSplit(split) = outcome else {
        panic!("ownership split must refuse confirmation");
    };
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
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
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
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect_err("an unsafe member must not be silently omitted");

    assert!(error.to_string().contains("symlinked SKILL.md"));
    assert!(!home.join("cache").exists(), "no Home cache may be created");
}

#[test]
fn default_policy_selects_the_highest_stable_semver_tag_when_no_release_exists() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let repo = committed_repo(workspace.path());
    git(&repo, &["tag", "v1.2.0"]);
    git(&repo, &["tag", "v2.0.0"]);
    git(&repo, &["tag", "v3.0.0-beta.1"]);
    git(&repo, &["tag", "not-a-version"]);
    let service = SourceGroupPreviewService::new(
        Arc::new(FixtureGitSource::new(&repo)),
        Arc::new(EmptyLocks),
    );
    let outcome = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: None,
        })
        .expect("default policy preview");

    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("a clean single source should preview");
    };
    assert_eq!(preview.policy.mode, "auto_release_tag_head");
    assert_eq!(preview.policy.selection_kind, "semver_tag");
    assert_eq!(preview.policy.selected_ref, "v2.0.0");
    assert!(preview.policy.resolved_commit.len() == 40);
    assert_eq!(preview.members.len(), 2);
}

#[test]
fn explicit_fixed_tag_override_selects_the_requested_release() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let repo = committed_repo(workspace.path());
    git(&repo, &["tag", "v1.2.0"]);
    git(&repo, &["tag", "v2.0.0"]);
    let service = SourceGroupPreviewService::new(
        Arc::new(FixtureGitSource::new(&repo)),
        Arc::new(EmptyLocks),
    );
    let outcome = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "fixed_tag".into(),
                value: Some("v1.2.0".into()),
            }),
        })
        .expect("fixed tag preview");

    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("a fixed tag always previews one release");
    };
    assert_eq!(preview.policy.selection_kind, "fixed_tag");
    assert_eq!(preview.policy.selected_ref, "v1.2.0");
}

#[test]
fn default_policy_falls_back_to_head_when_no_tag_is_reachable() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let repo = committed_repo(workspace.path());
    // An ordinary tag on an orphan branch is never reachable from the
    // default branch, so the reachable ordinary-tag step must skip it.
    git(&repo, &["checkout", "-q", "-b", "orphan"]);
    write_file(
        &repo,
        "orphan/SKILL.md",
        "---\nname: Orphan\n---\n# Orphan\n",
    );
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "orphan"]);
    git(&repo, &["tag", "orphan-release"]);
    git(&repo, &["checkout", "-q", "main"]);
    let service = SourceGroupPreviewService::new(
        Arc::new(FixtureGitSource::new(&repo)),
        Arc::new(EmptyLocks),
    );
    let outcome = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: None,
        })
        .expect("HEAD fallback preview");

    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("a source with only unreachable tags still previews");
    };
    assert_eq!(preview.policy.selection_kind, "head");
    assert_eq!(preview.policy.selected_ref, "HEAD");
    assert_eq!(preview.members.len(), 2);
}

#[test]
fn prerelease_channel_requires_an_explicit_channel_or_fails_closed() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let repo = committed_repo(workspace.path());
    git(&repo, &["tag", "v2.0.0-rc.1"]);
    let service = SourceGroupPreviewService::new(
        Arc::new(FixtureGitSource::new(&repo)),
        Arc::new(EmptyLocks),
    );
    let error = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "prerelease_channel".into(),
                value: Some("beta".into()),
            }),
        })
        .expect_err("an unfulfilled channel must stay closed");

    assert!(
        error.to_string().contains("prerelease"),
        "the channel failure is a typed policy error"
    );
    let outcome = service
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "prerelease_channel".into(),
                value: Some("rc".into()),
            }),
        })
        .expect("matching channel preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("a matching channel previews its release");
    };
    assert_eq!(preview.policy.selection_kind, "prerelease_channel");
    assert_eq!(preview.policy.selected_ref, "v2.0.0-rc.1");
}
