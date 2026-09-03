//! End-to-end remote Install and Update flows against local fixture
//! repositories (file:// transport), covering ADR-0004: two-phase discovery,
//! ref anchoring, grouped update checks with cooldown, stable replacement
//! that keeps Activations, Modified refusal, upstream path-gone, and pinning.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::git_source_capability::SqliteGitSourceCapabilityReader;
use skill_man_lib::adapters::local_file_source::LocalFileSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::domain::{CatalogFilter, Health, SourceKind};
use skill_man_lib::core::git_source_capability::{
    GitRepositorySourceFact, GitSourceCapabilityFacts, GitSourceCapabilityReader,
    GitSourceCapabilityScan, GitSourceCatalogStructure, GitSourceFact, GitSourceManifestFact,
    GitSourceReleaseFact,
};
use skill_man_lib::core::import::{ImportError, ImportService};
use skill_man_lib::core::update::{
    UpdateApplyRequest, UpdateError, UpdateSelection, UpdateService,
};
use skill_man_lib::core::write_gate::WriteGate;
use skill_man_lib::seams::git_source_capability::GitSourceMemberFact;
use skill_man_lib::seams::import_store::ImportStore;

mod common;
use common::{BoundTestHome, CATALOG_FILE_NAME};

#[derive(Clone)]
struct StaticGitSourceCapabilityReader {
    facts: GitSourceCapabilityFacts,
}

impl GitSourceCapabilityReader for StaticGitSourceCapabilityReader {
    fn read(&self) -> Result<GitSourceCapabilityFacts, String> {
        Ok(self.facts.clone())
    }
}

fn complete_repository_source_scan(
    remote_id: &str,
    canonical_url: &str,
) -> Arc<GitSourceCapabilityScan> {
    Arc::new(GitSourceCapabilityScan::new(Arc::new(
        StaticGitSourceCapabilityReader {
            facts: GitSourceCapabilityFacts {
                catalog_structure: GitSourceCatalogStructure {
                    has_repository_sources_table: true,
                    has_releases_table: true,
                    has_release_members_table: true,
                    has_members_table: true,
                    has_required_columns: true,
                    has_required_foreign_keys: true,
                    has_required_unique_constraints: true,
                    has_clean_foreign_key_check: true,
                    has_clean_integrity_check: true,
                },
                sources: vec![GitSourceFact {
                    remote_id: remote_id.into(),
                    canonical_url: canonical_url.into(),
                    catalog_aliases: vec![],
                    repository: Some(GitRepositorySourceFact {
                        provider: Some("generic".into()),
                        canonical_url: canonical_url.into(),
                        tracking_mode: Some("branch".into()),
                        tracking_value: Some("main".into()),
                        current_selected_ref: Some("main".into()),
                        current_release_id: Some("release-1".into()),
                        current_release: Some(GitSourceReleaseFact {
                            release_id: "release-1".into(),
                            remote_id: remote_id.into(),
                            selection_kind: "branch".into(),
                            selected_ref: "main".into(),
                            resolved_commit: "abc123".into(),
                            member_paths: vec!["skills/alpha".into()],
                        }),
                        current_members: vec![GitSourceMemberFact {
                            skill_id: "alpha-skill".into(),
                            skill_path: "skills/alpha".into(),
                            storage_relpath: format!("skills/git/{remote_id}/alpha-skill"),
                            presence: true,
                        }],
                    }),
                    manifest: GitSourceManifestFact::Present {
                        remote_id: remote_id.into(),
                        canonical_url: canonical_url.into(),
                        aliases: vec![],
                        provider: Some("generic".into()),
                        tracking_mode: Some("branch".into()),
                        tracking_value: Some("main".into()),
                        current_selected_ref: Some("main".into()),
                        current_release_id: Some("release-1".into()),
                    },
                }],
            },
        },
    )))
}

fn git(repo: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"]).trim().to_owned()
}

fn head_commit(repo: &Path) -> String {
    git(repo, &["rev-parse", "HEAD"]).trim().to_owned()
}

fn write_files(repo: &Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let file = repo.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).expect("create fixture parent");
        std::fs::File::create(&file)
            .expect("create fixture file")
            .write_all(contents.as_bytes())
            .expect("write fixture file");
    }
}

fn init_repo(parent: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let repo = parent.join(name);
    std::fs::create_dir_all(&repo).expect("create fixture repo");
    write_files(&repo, files);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "fixture@example.com"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    commit(&repo, "fixture commit");
    repo
}

fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

struct Harness {
    home: BoundTestHome,
    library_root: PathBuf,
    cache_root: PathBuf,
    runtime: Arc<RuntimeCatalogStore>,
    filesystem: Arc<MacOsFileSystem>,
}

impl Harness {
    fn new() -> Self {
        let home = BoundTestHome::new();
        home.seed_standard_library();
        let library_root = home.library_root.clone();
        let filesystem = home.filesystem.clone();
        let runtime = home.runtime.clone();
        let cache_root = library_root.join("cache");
        Self {
            home,
            library_root,
            cache_root,
            runtime,
            filesystem,
        }
    }

    fn import(&self) -> ImportService {
        ImportService::new(
            self.runtime.clone(),
            self.filesystem.clone(),
            Arc::new(SystemClock::new()),
            Arc::new(LocalFileSource::new()),
            self.library_root.clone(),
        )
        .with_git_source(Arc::new(SystemGitSource::new()))
        .with_git_cache_root(self.cache_root.clone())
    }

    fn update(&self) -> UpdateService {
        UpdateService::new(
            self.runtime.clone(),
            self.filesystem.clone(),
            Arc::new(SystemClock::new()),
            Arc::new(SystemGitSource::new()),
            self.library_root.clone(),
            self.cache_root.clone(),
            Arc::new(WriteGate::open_for_tests()),
        )
    }

    fn update_with_source_capability_scan(&self) -> UpdateService {
        self.update()
            .with_git_source_capability_scan(Arc::new(GitSourceCapabilityScan::new(Arc::new(
                SqliteGitSourceCapabilityReader::new(
                    self.home.write_gate.clone(),
                    CATALOG_FILE_NAME,
                    self.filesystem.clone(),
                ),
            ))))
    }

    fn catalog(&self) -> CatalogService {
        CatalogService::new(self.runtime.clone())
    }
}

#[test]
fn git_import_installs_multi_skill_repo_and_records_remote_source() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "fixture-repo",
        &[
            ("SKILL.md", "---\nname: root-skill\n---\n# Root\n"),
            ("skills/alpha/SKILL.md", "---\nname: alpha\n---\n# Alpha\n"),
            (
                "skills/beta/SKILL.md",
                "---\nname: beta\ndescription: Beta skill\n---\n# Beta\n",
            ),
        ],
    );
    let source = file_url(&repo);
    let import = harness.import();

    let discovered = import
        .discover_git(&source, false)
        .expect("discover Git source");
    let names = discovered
        .candidates
        .iter()
        .map(|candidate| candidate.directory_name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["alpha", "beta", "fixture-repo"]);
    assert_eq!(discovered.requested_ref, "HEAD");
    assert_eq!(discovered.resolved_commit, head_commit(&repo));
    assert_eq!(discovered.repo_url, source);
    assert!(!discovered.truncated);

    let preview = import
        .plan_git_selection(&source, false, &["alpha".into(), "beta".into()])
        .expect("plan Git selection");
    assert_eq!(preview.items.len(), 2);
    assert!(preview.can_apply);
    assert!(preview.items.iter().all(|item| {
        item.final_entity_path
            .starts_with(harness.library_root.join("skills"))
    }));

    let result = import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Git selection");
    assert_eq!(result.items.len(), 2);

    let installed_alpha = harness.library_root.join("skills/alpha");
    assert_eq!(
        std::fs::read_to_string(installed_alpha.join("SKILL.md")).expect("read alpha"),
        "---\nname: alpha\n---\n# Alpha\n"
    );
    let installs = harness
        .catalog()
        .list(CatalogFilter::Install)
        .expect("list Installs")
        .items;
    let installed = installs
        .iter()
        .filter(|skill| skill.source_kind == SourceKind::RemoteInstall)
        .collect::<Vec<_>>();
    let installed_names = installed
        .iter()
        .map(|skill| skill.directory_name.as_str())
        .collect::<Vec<_>>();
    assert!(
        installed_names.contains(&"alpha") && installed_names.contains(&"beta"),
        "remote Installs are visible in the catalog: {installed_names:?}"
    );

    let records = harness
        .runtime
        .load_remote_installs()
        .expect("load remote sources");
    assert_eq!(records.len(), 2);
    let alpha = records
        .iter()
        .find(|record| record.directory_name == "alpha")
        .expect("alpha record");
    assert_eq!(alpha.source_url, source);
    assert_eq!(alpha.requested_ref, "HEAD");
    assert_eq!(alpha.verification_anchor_commit, head_commit(&repo));
    assert_eq!(alpha.skill_path, "skills/alpha");
    assert!(alpha.last_updated_at.is_some());
    assert!(
        harness
            .cache_root
            .join("git")
            .read_dir()
            .expect("cache")
            .count()
            >= 1,
        "the mirror cache persists for update checks"
    );
}

#[test]
fn legacy_per_skill_git_state_cannot_fetch_plan_apply_or_pin_an_old_update() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "legacy-update-repo",
        &[("skills/alpha/SKILL.md", "# Alpha v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("plan the v6-style remote Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("create the v6-style remote Install");
    let skill_id = installed.items[0].skill_id.clone();
    let before = harness
        .runtime
        .load_remote_installs()
        .expect("read legacy record")[0]
        .clone();

    write_files(&repo, &[("skills/alpha/SKILL.md", "# Alpha v2\n")]);
    commit(&repo, "upstream moves while legacy remains closed");

    let update = harness.update_with_source_capability_scan();
    let report = update
        .check_updates(true)
        .expect("scan closes before fetch");
    assert!(
        report.groups.is_empty(),
        "legacy does not enter Update checks"
    );
    let after_check = harness
        .runtime
        .load_remote_installs()
        .expect("read after closed check")[0]
        .clone();
    assert_eq!(after_check.last_checked_at, before.last_checked_at);

    let plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("closed plan is an item result");
    assert_eq!(plan.items.len(), 1);
    assert_eq!(
        plan.items[0].error.as_deref(),
        Some(
            "Per-Skill Update is closed; Git Repository Source Update is not available in this release"
        )
    );

    let applied = update
        .apply_updates(
            &[UpdateApplyRequest {
                plan_token: "unexpected-plan".into(),
                skill_id: skill_id.clone(),
                directory_name: "alpha".into(),
            }],
            false,
        )
        .expect("closed apply returns an item failure");
    assert!(!applied.items[0].updated);
    assert_eq!(applied.items[0].error, plan.items[0].error);
    assert!(matches!(
        update.pin_updates(std::slice::from_ref(&skill_id)),
        Err(UpdateError::SourceCapabilityClosed)
    ));
    let after = harness
        .runtime
        .load_remote_installs()
        .expect("read after closed actions")[0]
        .clone();
    assert_eq!(after.requested_ref, before.requested_ref);
    assert_eq!(
        after.verification_anchor_commit,
        before.verification_anchor_commit
    );
}

#[test]
fn complete_repository_source_cannot_use_old_per_skill_update_commands() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "complete-source-update-repo",
        &[("skills/alpha/SKILL.md", "# Alpha v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("plan the historical remote Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("create the historical remote Install");
    let skill_id = installed.items[0].skill_id.clone();
    let before = harness
        .runtime
        .load_remote_installs()
        .expect("read historical record")[0]
        .clone();

    write_files(&repo, &[("skills/alpha/SKILL.md", "# Alpha v2\n")]);
    commit(&repo, "upstream moves while source Update is unavailable");

    let update = harness
        .update()
        .with_git_source_capability_scan(complete_repository_source_scan(
            &before.remote_id,
            &before.source_url,
        ));
    let report = update
        .check_updates(true)
        .expect("old Update API closes before fetch");
    assert!(
        report.groups.is_empty(),
        "a complete source still cannot enter the retired per-Skill Update API"
    );
    let after_check = harness
        .runtime
        .load_remote_installs()
        .expect("read after closed check")[0]
        .clone();
    assert_eq!(after_check.last_checked_at, before.last_checked_at);

    let plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("closed plan is an item result");
    assert_eq!(plan.items.len(), 1);
    assert_eq!(
        plan.items[0].error.as_deref(),
        Some(
            "Per-Skill Update is closed; Git Repository Source Update is not available in this release"
        )
    );

    let applied = update
        .apply_updates(
            &[UpdateApplyRequest {
                plan_token: "unexpected-plan".into(),
                skill_id: skill_id.clone(),
                directory_name: "alpha".into(),
            }],
            false,
        )
        .expect("closed apply returns an item failure");
    assert!(!applied.items[0].updated);
    assert_eq!(applied.items[0].error, plan.items[0].error);
    assert!(matches!(
        update.pin_updates(std::slice::from_ref(&skill_id)),
        Err(UpdateError::SourceCapabilityClosed)
    ));
    let after = harness
        .runtime
        .load_remote_installs()
        .expect("read after closed actions")[0]
        .clone();
    assert_eq!(after.requested_ref, before.requested_ref);
    assert_eq!(
        after.verification_anchor_commit,
        before.verification_anchor_commit
    );
}

#[test]
fn git_import_two_phase_discovery_falls_back_to_recursion() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "discovery-repo",
        &[
            ("skills/standard/SKILL.md", "# Standard\n"),
            ("packages/nested/deep/SKILL.md", "# Nested\n"),
        ],
    );
    let source = file_url(&repo);
    let import = harness.import();

    let standard = import
        .discover_git(&source, false)
        .expect("standard discovery");
    let names = standard
        .candidates
        .iter()
        .map(|candidate| candidate.directory_name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["standard"],
        "standard positions satisfy the scan"
    );

    let full = import
        .discover_git(&source, true)
        .expect("full-depth discovery");
    let mut full_names = full
        .candidates
        .iter()
        .map(|candidate| candidate.skill_path.as_str())
        .collect::<Vec<_>>();
    full_names.sort();
    assert_eq!(full_names, vec!["packages/nested/deep", "skills/standard"]);
}

#[test]
fn git_import_conflict_blocks_duplicate_identity() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "conflict-repo",
        &[("skills/alpha/SKILL.md", "# Alpha\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();

    let first = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("first plan");
    import
        .apply_git_selection(&first.plan_token)
        .expect("first apply");

    let second = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("second plan");
    assert!(!second.can_apply);
    let conflict = second.items[0].conflict.as_ref().expect("conflict");
    assert_eq!(conflict.directory_name, "alpha");
    let error = import
        .apply_git_selection(&second.plan_token)
        .expect_err("duplicate Import is rejected");
    assert!(matches!(error, ImportError::Conflict(_)));
}

#[test]
fn update_check_plan_apply_replaces_entity_at_the_new_commit() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "update-repo",
        &[("skills/alpha/SKILL.md", "# Alpha v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("plan Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Install");
    let skill_id = installed.items[0].skill_id.clone();
    let entity = harness.library_root.join("skills/alpha");
    let first_commit = head_commit(&repo);

    write_files(
        &repo,
        &[
            ("skills/alpha/SKILL.md", "# Alpha v2\n"),
            ("skills/alpha/notes.md", "new note\n"),
        ],
    );
    let second_commit = commit(&repo, "update alpha");

    let update = harness.update();
    let report = update.check_updates(true).expect("check for updates");
    assert!(report.errors.is_empty(), "{}", report.errors.join("; "));
    assert_eq!(report.groups.len(), 1);
    let item = &report.groups[0].items[0];
    assert_eq!(item.skill_id, skill_id);
    assert!(item.has_update);
    assert_eq!(item.current_commit, first_commit);
    assert_eq!(item.resolved_commit, second_commit);
    assert!(!item.modified);
    assert!(!item.upstream_path_gone);
    let plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("plan Update");
    assert_eq!(plan.items.len(), 1);
    assert!(plan.items[0].error.is_none(), "{:?}", plan.items[0].error);
    assert_eq!(plan.items[0].new_commit, second_commit);

    let applied = update
        .apply_updates(
            &[UpdateApplyRequest {
                plan_token: plan.items[0].plan_token.clone(),
                skill_id: skill_id.clone(),
                directory_name: "alpha".into(),
            }],
            false,
        )
        .expect("apply Update");
    assert!(applied.items[0].updated, "{:?}", applied.items[0].error);

    assert_eq!(
        std::fs::read_to_string(entity.join("SKILL.md")).expect("read updated SKILL.md"),
        "# Alpha v2\n"
    );
    assert!(entity.join("notes.md").is_file(), "new files arrive");
    let records = harness
        .runtime
        .load_remote_installs()
        .expect("reload remote sources");
    let alpha = records
        .iter()
        .find(|record| record.skill_id == skill_id)
        .expect("alpha record");
    assert_eq!(alpha.verification_anchor_commit, second_commit);
    assert_eq!(
        harness
            .catalog()
            .inspect(skill_id.clone())
            .expect("inspect")
            .summary
            .health,
        Health::Healthy
    );
}

#[test]
fn update_keeps_activations_pointing_at_the_stable_path() {
    let harness = Harness::new();
    let claude_root = harness.home.path().join(".claude/skills");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");
    let repo = init_repo(
        harness.home.path(),
        "activation-repo",
        &[("skills/alpha/SKILL.md", "# Alpha v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("plan Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Install");
    let skill_id = installed.items[0].skill_id.clone();
    let entity = harness.library_root.join("skills/alpha");

    let entry = claude_root.join("alpha");
    std::os::unix::fs::symlink(&entity, &entry).expect("create Target-scoped Activation entry");
    harness
        .home
        .seed_activation(&skill_id.0, "claude-code", true, "present");
    assert!(
        entry
            .symlink_metadata()
            .expect("entry")
            .file_type()
            .is_symlink()
    );

    write_files(&repo, &[("skills/alpha/SKILL.md", "# Alpha v2\n")]);
    let second_commit = commit(&repo, "update alpha");

    let update = harness.update();
    let plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("plan Update");
    assert!(plan.items[0].error.is_none(), "{:?}", plan.items[0].error);
    let applied = update
        .apply_updates(
            &[UpdateApplyRequest {
                plan_token: plan.items[0].plan_token.clone(),
                skill_id: skill_id.clone(),
                directory_name: "alpha".into(),
            }],
            false,
        )
        .expect("apply Update");
    assert!(applied.items[0].updated, "{:?}", applied.items[0].error);

    let target = std::fs::read_link(&entry).expect("read Activation target");
    assert_eq!(target, entity);
    assert_eq!(
        std::fs::read_to_string(entry.join("SKILL.md")).expect("read through Activation"),
        "# Alpha v2\n"
    );
    let records = harness
        .runtime
        .load_remote_installs()
        .expect("reload remote sources");
    assert_eq!(
        records
            .iter()
            .find(|record| record.skill_id == skill_id)
            .expect("record")
            .verification_anchor_commit,
        second_commit
    );
}

#[test]
fn update_refuses_modified_entity_without_abandoning_changes() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "modified-repo",
        &[("skills/alpha/SKILL.md", "# Alpha v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("plan Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Install");
    let skill_id = installed.items[0].skill_id.clone();
    let entity = harness.library_root.join("skills/alpha");
    let local_edit = entity.join("SKILL.md");
    std::fs::write(&local_edit, "# Alpha v1\n# local tweak\n").expect("local modification");

    write_files(&repo, &[("skills/alpha/SKILL.md", "# Alpha v2\n")]);
    commit(&repo, "update alpha");

    let update = harness.update();
    let plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("plan Update");
    assert!(plan.items[0].modified);

    let refused = update
        .apply_updates(
            &[UpdateApplyRequest {
                plan_token: plan.items[0].plan_token.clone(),
                skill_id: skill_id.clone(),
                directory_name: "alpha".into(),
            }],
            false,
        )
        .expect("apply without abandon");
    assert!(!refused.items[0].updated);
    let message = refused.items[0].error.as_deref().expect("error");
    assert!(message.contains("modifications"), "{message}");
    assert_eq!(
        std::fs::read_to_string(&local_edit).expect("local edit survives"),
        "# Alpha v1\n# local tweak\n"
    );

    let second_plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("replan Update");
    let applied = update
        .apply_updates(
            &[UpdateApplyRequest {
                plan_token: second_plan.items[0].plan_token.clone(),
                skill_id: skill_id.clone(),
                directory_name: "alpha".into(),
            }],
            true,
        )
        .expect("apply abandoning changes");
    assert!(applied.items[0].updated, "{:?}", applied.items[0].error);
    assert_eq!(
        std::fs::read_to_string(entity.join("SKILL.md")).expect("updated entity"),
        "# Alpha v2\n"
    );
}

#[test]
fn upstream_path_gone_is_reported_and_reselectable() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "relocation-repo",
        &[("packages/foo/SKILL.md", "# Foo v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let discovered = import
        .discover_git(&source, false)
        .expect("discover")
        .candidates;
    assert_eq!(discovered.len(), 1);
    let preview = import
        .plan_git_selection(&source, false, &["foo".into()])
        .expect("plan Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Install");
    let skill_id = installed.items[0].skill_id.clone();
    let entity = harness.library_root.join("skills/foo");

    write_files(&repo, &[("packages/bar/SKILL.md", "# Bar v2\n")]);
    let removed = Command::new("git")
        .args(["rm", "-r", "-q", "packages/foo"])
        .current_dir(&repo)
        .output()
        .expect("remove upstream path");
    assert!(removed.status.success());
    let new_commit = commit(&repo, "relocate foo to bar");

    let update = harness.update();
    let report = update.check_updates(true).expect("check");
    let item = &report.groups[0].items[0];
    assert!(item.has_update);
    assert!(item.upstream_path_gone);

    let stale_plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("plan stale path");
    let error = stale_plan.items[0]
        .error
        .as_deref()
        .expect("path-gone error");
    assert!(error.contains("no longer exists"), "{error}");

    let reselect_plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: Some("packages/bar".into()),
        }])
        .expect("plan reselected path");
    assert!(
        reselect_plan.items[0].error.is_none(),
        "{:?}",
        reselect_plan.items[0].error
    );
    assert!(reselect_plan.items[0].path_changed);
    let applied = update
        .apply_updates(
            &[UpdateApplyRequest {
                plan_token: reselect_plan.items[0].plan_token.clone(),
                skill_id: skill_id.clone(),
                directory_name: "foo".into(),
            }],
            false,
        )
        .expect("apply reselection");
    assert!(applied.items[0].updated, "{:?}", applied.items[0].error);
    assert_eq!(
        std::fs::read_to_string(entity.join("SKILL.md")).expect("relocated entity"),
        "# Bar v2\n"
    );
    let records = harness.runtime.load_remote_installs().expect("reload");
    let foo = records
        .iter()
        .find(|record| record.skill_id == skill_id)
        .expect("foo record");
    assert_eq!(foo.skill_path, "packages/bar");
    assert_eq!(foo.verification_anchor_commit, new_commit);
}

#[test]
fn pinned_and_commit_refs_are_never_checked() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "pinned-repo",
        &[("skills/alpha/SKILL.md", "# Alpha v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("plan Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Install");
    let skill_id = installed.items[0].skill_id.clone();
    let installed_commit = head_commit(&repo);

    write_files(&repo, &[("skills/alpha/SKILL.md", "# Alpha v2\n")]);
    commit(&repo, "upstream moves on");

    let update = harness.update();
    let before_pin_report = update.check_updates(true).expect("check before pin");
    let before_pin = before_pin_report
        .groups
        .iter()
        .flat_map(|group| group.items.iter())
        .find(|item| item.skill_id == skill_id)
        .expect("update is visible before pinning");
    assert!(before_pin.has_update);

    update
        .pin_updates(std::slice::from_ref(&skill_id))
        .expect("pin the Install");
    let records = harness.runtime.load_remote_installs().expect("reload");
    let alpha = records
        .iter()
        .find(|record| record.skill_id == skill_id)
        .expect("alpha record");
    assert_eq!(alpha.requested_ref, installed_commit);
    assert!(skill_man_lib::core::git_source::is_pinned_ref(
        &alpha.requested_ref
    ));
    let after_pin = update.check_updates(true).expect("check after pin");
    assert!(
        after_pin
            .groups
            .iter()
            .flat_map(|group| group.items.iter())
            .all(|item| !item.has_update),
        "pinned Installs are never checked"
    );
}

#[test]
fn cooldown_skips_recent_checks_until_forced() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "cooldown-repo",
        &[("skills/alpha/SKILL.md", "# Alpha v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("plan Install");
    import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Install");

    let update = harness.update();
    let forced = update.check_updates(true).expect("forced check");
    assert_eq!(forced.groups.len(), 1);

    let within_cooldown = update.check_updates(false).expect("cooldown check");
    assert!(
        within_cooldown.groups.is_empty(),
        "a successful check suppresses further checks for 24h"
    );

    let forced_again = update.check_updates(true).expect("forced again");
    assert_eq!(forced_again.groups.len(), 1);
}

#[test]
fn unreadable_source_fails_with_source_error() {
    let harness = Harness::new();
    let import = harness.import();
    let error = import
        .discover_git("file:///nonexistent/repo", false)
        .expect_err("missing repo fails discovery");
    assert!(
        matches!(error, ImportError::Source(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn branch_tracked_installs_resolve_their_own_branch_in_check_and_plan() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "branch-repo",
        &[("skills/alpha/SKILL.md", "# Alpha v1\n")],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into()])
        .expect("plan Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Install");
    let skill_id = installed.items[0].skill_id.clone();

    // Track the "dev" branch instead of the default branch, as a
    // `tree/<ref>/...` URL would have recorded.
    git(&repo, &["checkout", "-q", "-b", "dev"]);
    write_files(&repo, &[("skills/alpha/SKILL.md", "# Alpha dev\n")]);
    let dev_commit = commit(&repo, "dev moves on");
    git(&repo, &["checkout", "-q", "main"]);
    write_files(&repo, &[("skills/alpha/SKILL.md", "# Alpha main\n")]);
    commit(&repo, "main moves on differently");
    harness
        .runtime
        .set_remote_requested_ref(&skill_id, "dev")
        .expect("track the dev branch");

    let update = harness.update();
    let report = update.check_updates(true).expect("check");
    let item = report
        .groups
        .iter()
        .flat_map(|group| group.items.iter())
        .find(|item| item.skill_id == skill_id)
        .expect("checked item");
    assert!(item.has_update);
    assert_eq!(
        item.resolved_commit, dev_commit,
        "the check must resolve the tracked branch, not the default branch"
    );

    let plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("plan");
    assert!(plan.items[0].error.is_none(), "{:?}", plan.items[0].error);
    assert_eq!(
        plan.items[0].new_commit, dev_commit,
        "the plan must stage the tracked branch commit"
    );
}

#[test]
fn same_repo_updates_apply_as_a_batch_and_fail_independently() {
    let harness = Harness::new();
    let repo = init_repo(
        harness.home.path(),
        "batch-repo",
        &[
            ("skills/alpha/SKILL.md", "# Alpha v1\n"),
            ("skills/beta/SKILL.md", "# Beta v1\n"),
        ],
    );
    let source = file_url(&repo);
    let import = harness.import();
    let preview = import
        .plan_git_selection(&source, false, &["alpha".into(), "beta".into()])
        .expect("plan Install");
    let installed = import
        .apply_git_selection(&preview.plan_token)
        .expect("apply Install");
    assert_eq!(installed.items.len(), 2);
    let alpha_id = installed
        .items
        .iter()
        .find(|item| item.directory_name == "alpha")
        .expect("alpha")
        .skill_id
        .clone();
    let beta_id = installed
        .items
        .iter()
        .find(|item| item.directory_name == "beta")
        .expect("beta")
        .skill_id
        .clone();

    write_files(
        &repo,
        &[
            ("skills/alpha/SKILL.md", "# Alpha v2\n"),
            ("skills/beta/SKILL.md", "# Beta v2\n"),
        ],
    );
    commit(&repo, "both skills advance");
    std::fs::write(
        harness.library_root.join("skills/beta/SKILL.md"),
        "# Beta v2\n# local edit\n",
    )
    .expect("modify beta locally");

    let update = harness.update();
    let report = update.check_updates(true).expect("check");
    assert_eq!(report.groups.len(), 1, "one repository, one merged fetch");
    assert_eq!(report.groups[0].items.len(), 2);

    let plan = update
        .plan_updates(&[
            UpdateSelection {
                skill_id: alpha_id.clone(),
                new_skill_path: None,
            },
            UpdateSelection {
                skill_id: beta_id.clone(),
                new_skill_path: None,
            },
        ])
        .expect("plan batch");
    let alpha_plan = plan
        .items
        .iter()
        .find(|item| item.skill_id == alpha_id)
        .expect("alpha plan");
    assert!(alpha_plan.error.is_none(), "{:?}", alpha_plan.error);
    let beta_plan = plan
        .items
        .iter()
        .find(|item| item.skill_id == beta_id)
        .expect("beta plan");
    assert!(beta_plan.modified);

    let applied = update
        .apply_updates(
            &[
                UpdateApplyRequest {
                    plan_token: alpha_plan.plan_token.clone(),
                    skill_id: alpha_id.clone(),
                    directory_name: "alpha".into(),
                },
                UpdateApplyRequest {
                    plan_token: beta_plan.plan_token.clone(),
                    skill_id: beta_id.clone(),
                    directory_name: "beta".into(),
                },
            ],
            false,
        )
        .expect("apply batch");
    let alpha_result = applied
        .items
        .iter()
        .find(|item| item.skill_id == alpha_id)
        .expect("alpha result");
    assert!(alpha_result.updated, "{:?}", alpha_result.error);
    let beta_result = applied
        .items
        .iter()
        .find(|item| item.skill_id == beta_id)
        .expect("beta result");
    assert!(
        !beta_result.updated,
        "Modified beta must refuse without abandon"
    );
    assert_eq!(
        std::fs::read_to_string(harness.library_root.join("skills/alpha/SKILL.md"))
            .expect("alpha updated"),
        "# Alpha v2\n"
    );
    assert_eq!(
        std::fs::read_to_string(harness.library_root.join("skills/beta/SKILL.md"))
            .expect("beta untouched"),
        "# Beta v2\n# local edit\n"
    );
}
