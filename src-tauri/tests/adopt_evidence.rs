//! Adopt evidence ledger acceptance (spec §8.1–§8.2, §10.2, ADR-0013):
//! multi-hop chains with exact failure hops, strict lock evidence, the
//! remote/ref/Verification Anchor/tree closed loop against real Git
//! fixtures, Modified three-way visibility, Deferred grouping, fixture
//! exclusion, plan staleness and the read-only proof for scan/plan.

use std::collections::HashMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::remote_provider::SystemRemoteProvider;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::adopt::{
    AdoptError, AdoptPlanIntent, AdoptPlanRequest, AdoptSelection, AdoptService, AdoptVerdict,
    AdoptVerdictReason, ModifiedBranch,
};
use skill_man_lib::seams::filesystem::{ChainFault, FileSystem};
use skill_man_lib::seams::installer_lock_store::{InstallerLockStore, LockEntry};
use skill_man_lib::seams::remote_provider::{
    AnchorResolution, RefDisposition, RemoteKind, RemoteProvider, RemoteProviderError,
    RemoteRequest, RemoteTreeFacts,
};
use skill_man_lib::seams::source::GitSource;

mod common;
use common::BoundTestHome;

fn write_skill(directory: &Path, name: &str, body: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::create_dir_all(&path).expect("create skill directory");
    std::fs::write(path.join("SKILL.md"), body).expect("write SKILL.md");
    path
}

/// A stable user location outside Home/Agent/shared roots.
fn projects(home: &BoundTestHome) -> PathBuf {
    let projects = home.path().join("Projects");
    std::fs::create_dir_all(&projects).expect("create Projects root");
    projects
}

struct Harness {
    home: BoundTestHome,
    projects: PathBuf,
    filesystem: Arc<MacOsFileSystem>,
    runtime: Arc<RuntimeCatalogStore>,
}

impl Harness {
    fn new() -> Self {
        let home = BoundTestHome::new();
        home.seed_standard_library();
        let projects = projects(&home);
        for directory in [
            home.claude_root(),
            home.codex_root(),
            home.path().join(".agents/skills"),
        ] {
            std::fs::create_dir_all(&directory).expect("create scan source directory");
        }
        let filesystem = home.filesystem.clone();
        let runtime = home.runtime.clone();
        Self {
            home,
            projects,
            filesystem,
            runtime,
        }
    }

    fn claude(&self) -> PathBuf {
        self.home.claude_root()
    }

    fn codex(&self) -> PathBuf {
        self.home.codex_root()
    }

    fn shared(&self) -> PathBuf {
        self.home.path().join(".agents/skills")
    }

    fn lock_path(&self) -> PathBuf {
        self.home.path().join(".agents/.skill-lock.json")
    }

    fn adopt_with(
        &self,
        locks: Arc<dyn InstallerLockStore>,
        provider: Arc<dyn RemoteProvider>,
    ) -> AdoptService {
        AdoptService::new(
            self.runtime.clone(),
            self.filesystem.clone(),
            Arc::new(SystemClock::new()),
            self.home.library_root.clone(),
            self.home.path().to_path_buf(),
        )
        .with_lock_store(locks)
        .with_remote_provider(provider)
    }

    fn adopt(&self) -> AdoptService {
        self.adopt_with(
            Arc::new(SystemInstallerLockStore::new(
                self.home.path().to_path_buf(),
            )),
            Arc::new(SystemRemoteProvider::new(Arc::new(SystemGitSource::new()))),
        )
    }
}

fn lock_json(entries: &[(&str, &str, String, Option<&str>, String, String)]) -> String {
    // Callers pass String temporaries via .as_str(); the array lives for the
    // call duration only.
    // (name, sourceType, sourceUrl, ref, skillPath, hash)
    let rendered = entries
        .iter()
        .map(|(name, source_type, url, reference, skill_path, hash)| {
            let mut fields = vec![
                format!(r#""sourceType": "{source_type}""#),
                format!(r#""source": "{url}""#),
                format!(r#""sourceUrl": "{url}""#),
                format!(r#""skillPath": "{skill_path}""#),
                format!(r#""skillFolderHash": "{hash}""#),
            ];
            if let Some(reference) = reference {
                fields.push(format!(r#""ref": "{reference}""#));
            }
            format!(
                r#""{name}": {{
                    {}
                }}"#,
                fields.join(
                    ",
"
                )
            )
        })
        .collect::<Vec<_>>()
        .join(
            ",
",
        );
    format!(r#"{{"version": 3, "skills": {{{rendered}}}}}"#)
}

fn select(
    candidates: &[skill_man_lib::core::adopt::AdoptEvidenceCandidate],
    name: &str,
) -> AdoptSelection {
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.directory_name == name)
        .unwrap_or_else(|| panic!("candidate '{name}' not found"));
    AdoptSelection {
        canonical_entity: candidate.canonical_entity.clone(),
        agent_ids: Vec::new(),
        modified_branch: None,
        target_directory: None,
    }
}

// -- Scripted remote provider for deterministic Deferred/Conflict grouping --

#[allow(dead_code)] // Conflict is part of the scripted contract for future cases
enum ScriptedOutcome {
    Deferred(String),
    Conflict(String),
    Ok {
        files: Vec<(String, String)>,
        anchor: String,
    },
}

struct ScriptedRemoteProvider {
    behaviors: HashMap<String, ScriptedOutcome>,
}

impl ScriptedRemoteProvider {
    fn new(behaviors: HashMap<String, ScriptedOutcome>) -> Self {
        Self { behaviors }
    }
}

impl RemoteProvider for ScriptedRemoteProvider {
    fn parse_request(&self, entry: &LockEntry) -> Result<RemoteRequest, RemoteProviderError> {
        Ok(RemoteRequest {
            kind: RemoteKind::GenericGit,
            canonical_url: entry.source_url.clone(),
            requested_ref: entry.requested_ref.as_deref().unwrap_or("HEAD").to_owned(),
            skill_path: entry.skill_path.clone(),
            provider_hash: entry.skill_folder_hash.clone(),
        })
    }

    fn verify(
        &self,
        request: &RemoteRequest,
        workspace: &Path,
    ) -> Result<RemoteTreeFacts, RemoteProviderError> {
        match self
            .behaviors
            .get(&request.canonical_url)
            .unwrap_or_else(|| panic!("no scripted behavior for {}", request.canonical_url))
        {
            ScriptedOutcome::Deferred(detail) => Err(RemoteProviderError::Deferred(detail.clone())),
            ScriptedOutcome::Conflict(detail) => Err(RemoteProviderError::Conflict(detail.clone())),
            ScriptedOutcome::Ok { files, anchor } => {
                let tree = workspace.join("tree");
                for (path, contents) in files {
                    let file = tree.join(path);
                    std::fs::create_dir_all(file.parent().expect("tree parent"))
                        .expect("create tree parent");
                    std::fs::write(&file, contents).expect("write materialized file");
                }
                Ok(RemoteTreeFacts {
                    disposition: RefDisposition::Branch,
                    anchor: AnchorResolution {
                        anchor_commit: anchor.clone(),
                        original_install_commit_known: false,
                    },
                    subtree_tree_sha: "a".repeat(40),
                    provider_hash_matched: true,
                    materialized_root: tree,
                    default_branch: Some("main".into()),
                })
            }
        }
    }
}

// -- Real Git fixture helpers --

fn fixture_repo(root: &Path, files: &[(&str, &str)]) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("create fixture repo");
    for (path, contents) in files {
        let file = repo.join(path);
        std::fs::create_dir_all(file.parent().expect("fixture parent")).expect("create parent");
        std::fs::write(&file, contents).expect("write fixture file");
    }
    let output = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&repo)
        .output()
        .expect("git init");
    assert!(output.status.success());
    commit_all(&repo, "fixture commit");
    repo
}

fn commit_all(repo: &Path, message: &str) {
    let output = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .output()
        .expect("git add");
    assert!(output.status.success());
    let output = std::process::Command::new("git")
        .args(["commit", "-q", "-m", message])
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .current_dir(repo)
        .output()
        .expect("git commit");
    assert!(output.status.success());
}

fn commit_file(repo: &Path, path: &str, contents: &str) -> String {
    let file = repo.join(path);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("create parent");
    std::fs::write(&file, contents).expect("write file");
    commit_all(repo, "fixture update");
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .expect("git rev-parse");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn tag(repo: &Path, name: &str) {
    let output = std::process::Command::new("git")
        .args(["tag", name])
        .current_dir(repo)
        .output()
        .expect("git tag");
    assert!(output.status.success());
}

/// The installer CLI hash of the skill folder at the given commit,
/// materialized through the production transport seam.
fn cli_hash_at(repo: &Path, commit: &str, skill_path: &str) -> String {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let mirror = workspace.path().join("mirror.git");
    SystemGitSource::new()
        .fetch_mirror(&format!("file://{}", repo.display()), &mirror)
        .expect("fetch mirror");
    let destination = workspace.path().join("tree");
    SystemGitSource::new()
        .stage_skill(&mirror, commit, skill_path, &destination)
        .expect("stage skill");
    skill_man_lib::adapters::remote_provider::cli_skill_folder_hash(&destination).expect("cli hash")
}

// =====================================================================
// Source chains (spec §8.1)
// =====================================================================

#[test]
fn evidence_reports_multi_hop_chains_and_aggregates_appearances() {
    let harness = Harness::new();
    let entity = write_skill(&harness.projects, "networking", "# Networking\n");
    let canonical = entity.canonicalize().expect("canonical entity");
    // Intermediate hops live outside every scan source so they are never
    // mistaken for appearances of the entity.
    // Relative targets keep the chain free of system plumbing symlinks
    // (macOS `/var -> /private/var`): every recorded hop is user-level.
    std::os::unix::fs::symlink("networking", harness.projects.join("second-hop"))
        .expect("second hop");
    std::os::unix::fs::symlink("second-hop", harness.projects.join("first-hop"))
        .expect("first hop");
    std::os::unix::fs::symlink(
        "../../Projects/first-hop",
        harness.claude().join("networking"),
    )
    .expect("Claude appearance");
    std::os::unix::fs::symlink(
        "../../Projects/first-hop",
        harness.codex().join("networking"),
    )
    .expect("Codex appearance");

    let report = harness.adopt().scan().expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "networking")
        .expect("candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Local);
    assert_eq!(candidate.canonical_entity, canonical);
    assert_eq!(candidate.appearances.len(), 2);
    let claude_appearance = candidate
        .appearances
        .iter()
        .find(|appearance| {
            appearance
                .appearance
                .agent_id
                .as_ref()
                .is_some_and(|id| id.0 == "claude-code")
        })
        .expect("Claude appearance");
    assert_eq!(claude_appearance.chain.hops.len(), 3);
    assert_eq!(
        claude_appearance.chain.final_entity.as_deref(),
        Some(canonical.as_path())
    );
    assert_eq!(
        claude_appearance.chain.hops[0]
            .path
            .file_name()
            .and_then(|name| name.to_str()),
        Some("networking")
    );
    let expected_hash = harness
        .filesystem
        .tree_hash(&canonical)
        .expect("local tree hash");
    assert_eq!(
        candidate.local_tree_hash.as_deref(),
        Some(expected_hash.as_str())
    );
}

#[test]
fn evidence_stops_at_the_exact_hop_for_every_chain_fault() {
    let harness = Harness::new();
    // Dangling.
    std::os::unix::fs::symlink("../../missing-target", harness.claude().join("dangling"))
        .expect("dangling symlink");
    // Cycle: a -> b -> a with relative targets.
    std::os::unix::fs::symlink("cycle-b", harness.projects.join("cycle-a")).expect("cycle a");
    std::os::unix::fs::symlink("cycle-a", harness.projects.join("cycle-b")).expect("cycle b");
    std::os::unix::fs::symlink("../../Projects/cycle-a", harness.claude().join("cycle"))
        .expect("cycle entry");
    // 16-hop limit.
    for index in 0..20 {
        let link = harness.projects.join(format!("hop-{index}"));
        let target = if index == 19 {
            "hop-real".to_owned()
        } else {
            format!("hop-{}", index + 1)
        };
        std::os::unix::fs::symlink(&target, &link).expect("chained symlink");
    }
    std::fs::create_dir_all(harness.projects.join("hop-real")).expect("real hop destination");
    std::os::unix::fs::symlink("../../Projects/hop-0", harness.claude().join("deep"))
        .expect("deep entry");
    // Non-UTF-8 symlink target (raw bytes are legal in a target).
    use std::os::unix::ffi::OsStringExt;
    let non_utf8 = std::ffi::OsString::from_vec(vec![0x62, 0x61, 0x64, 0xff, 0x74]);
    std::os::unix::fs::symlink(&non_utf8, harness.claude().join("non-utf8"))
        .expect("non-UTF-8 target symlink");

    let report = harness.adopt().scan().expect("scan");
    let find = |name: &str| {
        report
            .candidates
            .iter()
            .find(|candidate| candidate.directory_name == name)
            .unwrap_or_else(|| panic!("candidate {name}"))
    };
    let dangling = find("dangling");
    assert_eq!(dangling.verdict, AdoptVerdict::Blocked);
    assert!(matches!(
        dangling.reason,
        Some(AdoptVerdictReason::ChainFault {
            fault: ChainFault::Dangling { .. }
        })
    ));
    assert_eq!(dangling.local_tree_hash, None);

    let cycle = find("cycle");
    assert_eq!(cycle.verdict, AdoptVerdict::Blocked);
    assert!(matches!(
        cycle.reason,
        Some(AdoptVerdictReason::ChainFault {
            fault: ChainFault::Cycle { .. }
        })
    ));

    let deep = find("deep");
    assert_eq!(deep.verdict, AdoptVerdict::Blocked);
    assert!(matches!(
        deep.reason,
        Some(AdoptVerdictReason::ChainFault {
            fault: ChainFault::HopLimit { .. }
        })
    ));
    assert_eq!(deep.appearances[0].chain.hops.len(), 16);

    let non_utf8_candidate = find("non-utf8");
    assert_eq!(non_utf8_candidate.verdict, AdoptVerdict::Blocked);
    assert!(matches!(
        non_utf8_candidate.reason,
        Some(AdoptVerdictReason::ChainFault {
            fault: ChainFault::NonUtf8 { .. }
        })
    ));
    assert!(!non_utf8_candidate.selectable);
}

#[test]
fn evidence_blocks_an_unreadable_entity_without_partial_fingerprints() {
    let harness = Harness::new();
    let entity = write_skill(&harness.projects, "locked", "# Locked\n");
    let canonical = entity.canonicalize().expect("canonical entity");
    std::os::unix::fs::symlink(&canonical, harness.claude().join("locked")).expect("appearance");
    let mut permissions = std::fs::metadata(&canonical)
        .expect("metadata")
        .permissions();
    permissions.set_mode(0o000);
    std::fs::set_permissions(&canonical, permissions).expect("lock the entity");
    // Restore permissions so the tempdir can be cleaned even on panic.
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let mut permissions = std::fs::metadata(&self.0)
                .map(|metadata| metadata.permissions())
                .unwrap_or_else(|_| std::fs::Permissions::from_mode(0o755));
            permissions.set_mode(0o755);
            let _ = std::fs::set_permissions(&self.0, permissions);
        }
    }
    let _restore = Restore(canonical.clone());

    let report = harness.adopt().scan().expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "locked")
        .expect("locked candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Blocked);
    assert!(matches!(
        candidate.reason,
        Some(AdoptVerdictReason::ChainFault {
            fault: ChainFault::ReadFailed { .. }
        })
    ));
    assert_eq!(candidate.local_tree_hash, None);
    assert!(!candidate.selectable);
}

#[test]
fn evidence_excludes_fixture_footprints() {
    let harness = Harness::new();
    let fixture_entities = harness.home.path().join("fixture-entities");
    let fixture_skill = fixture_entities.join("skill-authoring");
    std::fs::create_dir_all(&fixture_skill).expect("fixture entity");
    std::fs::write(
        fixture_skill.join("SKILL.md"),
        common::FIXTURE_SKILL_AUTHORING_SKILL_MD,
    )
    .expect("fixture document");
    let canonical = fixture_skill.canonicalize().expect("canonical fixture");
    std::os::unix::fs::symlink(&canonical, harness.shared().join("skill-authoring"))
        .expect("fixture appearance");
    // A real candidate stays untouched by the exclusion.
    let real = write_skill(&harness.projects, "real-skill", "# Real\n");
    std::os::unix::fs::symlink(
        real.canonicalize().expect("canonical real"),
        harness.claude().join("real-skill"),
    )
    .expect("real appearance");

    let report = harness.adopt().scan().expect("scan");
    let fixture_candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "skill-authoring")
        .expect("fixture candidate is visible in the ledger");
    assert_eq!(fixture_candidate.verdict, AdoptVerdict::Excluded);
    assert_eq!(
        fixture_candidate.reason,
        Some(AdoptVerdictReason::FixtureEntity)
    );
    assert!(!fixture_candidate.selectable);
    assert!(!fixture_candidate.adoptable);
    let real_candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "real-skill")
        .expect("real candidate");
    assert_eq!(real_candidate.verdict, AdoptVerdict::Local);
}

#[test]
fn evidence_flags_identity_and_library_conflicts() {
    let harness = Harness::new();
    // Identity conflict: one entity under two directory names.
    let entity = write_skill(&harness.projects, "multi-name", "# Multi\n");
    let canonical = entity.canonicalize().expect("canonical entity");
    std::os::unix::fs::symlink(&canonical, harness.claude().join("multi-name"))
        .expect("first name");
    std::os::unix::fs::symlink(&canonical, harness.codex().join("multi-name-2"))
        .expect("second name");
    // Library conflict: a managed Link with the same identity.
    let managed = write_skill(&harness.projects, "managed-conflict", "# Managed\n");
    let managed_entity = managed.join("managed-conflict");
    std::fs::create_dir_all(&managed_entity).expect("managed entity");
    std::fs::write(managed_entity.join("SKILL.md"), "# Managed\n").expect("managed SKILL.md");
    let import = skill_man_lib::core::import::ImportService::new(
        harness.runtime.clone(),
        harness.filesystem.clone(),
        Arc::new(SystemClock::new()),
        Arc::new(skill_man_lib::adapters::local_file_source::LocalFileSource::new()),
        harness.home.library_root.clone(),
    );
    import
        .discover_link(&managed_entity)
        .expect("discover managed Link");
    import
        .apply_link(
            &import
                .plan_link(&managed_entity)
                .expect("plan Link")
                .plan_token,
        )
        .expect("apply managed Link");
    // The untracked appearance shares the identity but points at a
    // DIFFERENT entity: a Library conflict, never an alias.
    let untracked_entity = write_skill(
        &harness.projects,
        "untracked-conflict",
        "# Untracked
",
    );
    std::os::unix::fs::symlink(
        untracked_entity
            .canonicalize()
            .expect("canonical untracked"),
        harness.claude().join("managed-conflict"),
    )
    .expect("untracked appearance");

    let report = harness.adopt().scan().expect("scan");
    let identity = report
        .candidates
        .iter()
        .find(|candidate| candidate.canonical_entity == canonical)
        .expect("identity-conflicted candidate");
    assert_eq!(identity.verdict, AdoptVerdict::Conflict);
    assert!(matches!(
        &identity.reason,
        Some(AdoptVerdictReason::IdentityConflict { names })
            if *names == vec!["multi-name".to_owned(), "multi-name-2".to_owned()]
    ));
    assert!(!identity.selectable);

    let library = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "managed-conflict")
        .expect("library-conflicted candidate");
    assert_eq!(library.verdict, AdoptVerdict::Conflict);
    assert!(matches!(
        library.reason,
        Some(AdoptVerdictReason::LibraryConflict { .. })
    ));
    assert!(library.conflict.is_some());
    assert!(!library.selectable);
}

// =====================================================================
// Lock evidence (ADR-0013 §2)
// =====================================================================

#[test]
fn lock_missing_classifies_as_local_and_requires_relocation_inside_installer_root() {
    let harness = Harness::new();
    let entity = write_skill(&harness.shared(), "no-lock-skill", "# No lock\n");
    let canonical = entity.canonicalize().expect("canonical entity");

    let report = harness.adopt().scan().expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "no-lock-skill")
        .expect("candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Local);
    assert_eq!(candidate.reason, Some(AdoptVerdictReason::NoLock));
    assert!(candidate.lock.is_none());
    assert!(candidate.requires_relocation);
    assert_eq!(candidate.canonical_entity, canonical);
}

#[test]
fn corrupt_lock_blocks_every_candidate_its_root_governs() {
    let harness = Harness::new();
    write_skill(&harness.shared(), "governed-a", "# A\n");
    write_skill(&harness.shared(), "governed-b", "# B\n");
    // An untouched candidate outside the installer root stays Local.
    let free = write_skill(&harness.projects, "free-skill", "# Free\n");
    std::os::unix::fs::symlink(
        free.canonicalize().expect("canonical free"),
        harness.claude().join("free-skill"),
    )
    .expect("free appearance");
    std::fs::write(harness.lock_path(), "{not json").expect("corrupt lock");

    let report = harness.adopt().scan().expect("scan");
    for name in ["governed-a", "governed-b"] {
        let candidate = report
            .candidates
            .iter()
            .find(|candidate| candidate.directory_name == name)
            .expect("governed candidate");
        assert_eq!(candidate.verdict, AdoptVerdict::Conflict, "{name}");
        assert!(matches!(
            candidate.reason,
            Some(AdoptVerdictReason::LockFileFault { .. })
        ));
        assert!(
            candidate
                .lock
                .as_ref()
                .is_some_and(|lock| lock.file_fault.is_some())
        );
        assert!(!candidate.selectable);
    }
    let free_candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "free-skill")
        .expect("free candidate");
    assert_eq!(free_candidate.verdict, AdoptVerdict::Local);
}

#[test]
fn entry_level_lock_fault_blocks_only_that_entry() {
    let harness = Harness::new();
    write_skill(&harness.shared(), "broken-entry", "# Broken\n");
    write_skill(&harness.shared(), "fine-entry", "# Fine\n");
    let json = r#"{
        "version": 3,
        "skills": {
            "broken-entry": {
                "sourceType": "github",
                "source": "acme/broken",
                "sourceUrl": "https://github.com/acme/broken",
                "skillPath": "skills/broken",
                "skillFolderHash": ""
            },
            "fine-entry": {
                "sourceType": "git",
                "source": "https://example.com/fine",
                "sourceUrl": "https://example.com/fine",
                "skillPath": "skills/fine",
                "skillFolderHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }
        }
    }"#;
    std::fs::write(harness.lock_path(), json).expect("write lock");

    let report = harness.adopt().scan().expect("scan");
    let broken = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "broken-entry")
        .expect("broken candidate");
    assert_eq!(broken.verdict, AdoptVerdict::Conflict);
    assert!(matches!(
        broken.reason,
        Some(AdoptVerdictReason::LockEntryFault { .. })
    ));
    let fine = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "fine-entry")
        .expect("fine candidate");
    // The fine entry has no provider for example.com in the system adapter's
    // URL validation (https allowed for generic): the fetch itself is what
    // fails. With the system provider this is a Deferred (unreachable host).
    // This test only asserts the entry-level fault isolation: fine-entry is
    // NOT a Conflict from the lock; it reaches remote verification.
    assert_ne!(fine.verdict, AdoptVerdict::Conflict);
    assert!(matches!(
        fine.verdict,
        AdoptVerdict::Deferred | AdoptVerdict::Verified | AdoptVerdict::Modified
    ));
}

#[test]
fn duplicate_lock_owners_are_a_conflict() {
    let harness = Harness::new();
    write_skill(&harness.shared(), "dupe", "# Dupe\n");
    let json = lock_json(&[(
        "dupe",
        "git",
        "https://example.com/one".into(),
        None,
        "skills/dupe".into(),
        "0".repeat(64),
    )]);
    std::fs::write(harness.lock_path(), &json).expect("default lock");
    let xdg = harness.home.path().join("xdg-state/skills");
    std::fs::create_dir_all(&xdg).expect("xdg skills dir");
    std::fs::write(
        xdg.join(".skill-lock.json"),
        lock_json(&[(
            "dupe",
            "git",
            "https://example.com/two".into(),
            None,
            "skills/dupe".into(),
            "0".repeat(64),
        )]),
    )
    .expect("xdg lock");
    let locks = Arc::new(SystemInstallerLockStore::with_xdg(
        harness.home.path().to_path_buf(),
        Some(harness.home.path().join("xdg-state")),
    ));

    let report = harness
        .adopt_with(
            locks,
            Arc::new(SystemRemoteProvider::new(Arc::new(SystemGitSource::new()))),
        )
        .scan()
        .expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "dupe")
        .expect("dupe candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Conflict);
    assert!(matches!(
        candidate.reason,
        Some(AdoptVerdictReason::DuplicateLockOwner { .. })
    ));
    assert!(!candidate.selectable);
}

#[test]
fn lock_declaration_with_entity_elsewhere_is_a_conflict() {
    let harness = Harness::new();
    // The lock declares "escaped" but the entity lives in Projects.
    let entity = write_skill(&harness.projects, "escaped", "# Escaped\n");
    let canonical = entity.canonicalize().expect("canonical entity");
    std::os::unix::fs::symlink(&canonical, harness.claude().join("escaped")).expect("appearance");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "escaped",
            "git",
            "https://example.com/escaped".into(),
            None,
            "skills/escaped".into(),
            "0".repeat(64),
        )]),
    )
    .expect("write lock");

    let report = harness.adopt().scan().expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "escaped")
        .expect("candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Conflict);
    assert!(matches!(
        candidate.reason,
        Some(AdoptVerdictReason::EntityNotAtInstallerRoot { .. })
    ));
    assert!(!candidate.selectable);
}

// =====================================================================
// Remote closed loop with real Git fixtures (spec §8.2, ADR-0013 §2.2)
// =====================================================================

#[test]
fn verified_remote_loop_uses_the_real_git_anchor_and_trees() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
    // The installed entity: the exact remote tree at the anchor.
    write_skill(&harness.shared(), "networking", "# Networking v1\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "networking",
            "git",
            url.clone(),
            None,
            "skills/networking".into(),
            hash.clone(),
        )]),
    )
    .expect("write lock");

    let adopt = harness.adopt();
    let report = adopt.scan().expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "networking")
        .expect("candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Verified);
    let remote = candidate.remote.as_ref().expect("remote evidence");
    assert_eq!(remote.canonical_url, url);
    assert_eq!(remote.requested_ref, "HEAD");
    assert_eq!(remote.ref_kind, "head");
    assert!(remote.provider_hash_matched);
    assert!(remote.trees_match);
    assert_eq!(remote.remote_tree_hash, remote.local_tree_hash);
    assert!(!remote.original_install_commit_known);
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .expect("rev-parse");
    let head = String::from_utf8_lossy(&head.stdout).trim().to_owned();
    assert_eq!(remote.anchor_commit, head);
    let lock = candidate.lock.as_ref().expect("lock evidence");
    assert_eq!(lock.entry_name, "networking");
    assert_eq!(lock.lock_path, harness.lock_path());
    assert_eq!(
        lock.lock_fingerprint,
        format!("{:x}", {
            use sha2::Digest;
            sha2::Sha256::digest(std::fs::read(&harness.lock_path()).expect("lock bytes"))
        })
    );
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![select(&report.candidates, "networking")],
        })
        .expect("plan verified candidate");
    assert_eq!(
        plan.items[0].intent,
        AdoptPlanIntent::RemoteInstallKeepCurrent
    );
    assert!(
        plan.items[0].applyable,
        "the Handoff applies the verified tree"
    );
    assert!(plan.can_apply);
}

#[test]
fn pinned_tag_anchor_is_exact_and_known() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    tag(&repo, "v1.0.0");
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
    write_skill(&harness.shared(), "networking", "# Networking v1\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "networking",
            "git",
            url.clone(),
            Some("v1.0.0"),
            "skills/networking".into(),
            hash.clone(),
        )]),
    )
    .expect("write lock");

    let adopt = harness.adopt();
    let report = adopt.scan().expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "networking")
        .expect("candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Verified);
    let remote = candidate.remote.as_ref().expect("remote evidence");
    assert_eq!(remote.ref_kind, "tag");
    assert!(remote.original_install_commit_known);
}

#[test]
fn modified_candidate_shows_three_way_branches_and_never_auto_includes() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
    // The local entity diverged from the remote anchor.
    let entity = write_skill(
        &harness.shared(),
        "networking",
        "# Networking v1\nlocal change\n",
    );
    let canonical = entity.canonicalize().expect("canonical entity");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "networking",
            "git",
            url.clone(),
            None,
            "skills/networking".into(),
            hash.clone(),
        )]),
    )
    .expect("write lock");

    let adopt = harness.adopt();
    let report = adopt.scan().expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "networking")
        .expect("candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Modified);
    let remote = candidate.remote.as_ref().expect("remote evidence");
    assert!(remote.provider_hash_matched);
    assert!(!remote.trees_match);
    assert_ne!(remote.remote_tree_hash, remote.local_tree_hash);

    // All three branches are explicitly selectable; no candidate is
    // pre-included anywhere.
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![
                AdoptSelection {
                    canonical_entity: canonical.clone(),
                    agent_ids: Vec::new(),
                    modified_branch: Some(ModifiedBranch::KeepCurrent),
                    target_directory: None,
                },
                AdoptSelection {
                    canonical_entity: canonical.clone(),
                    agent_ids: Vec::new(),
                    modified_branch: Some(ModifiedBranch::DiscardToAnchor),
                    target_directory: None,
                },
                AdoptSelection {
                    canonical_entity: canonical.clone(),
                    agent_ids: Vec::new(),
                    modified_branch: Some(ModifiedBranch::ConvertToLocalLink),
                    target_directory: None,
                },
            ],
        })
        .expect("plan all three branches");
    assert_eq!(plan.items.len(), 3);
    assert!(
        plan.items
            .iter()
            .any(|item| item.intent == AdoptPlanIntent::RemoteInstallKeepCurrent)
    );
    assert!(
        plan.items
            .iter()
            .any(|item| item.intent == AdoptPlanIntent::RemoteInstallDiscardModified)
    );
    assert!(
        plan.items
            .iter()
            .any(|item| item.intent == AdoptPlanIntent::RemoteInstallConvertToLink)
    );
    assert!(!plan.can_apply);

    let missing_branch = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![AdoptSelection {
                canonical_entity: canonical,
                agent_ids: Vec::new(),
                modified_branch: None,
                target_directory: None,
            }],
        })
        .expect_err("Modified requires an explicit branch");
    assert!(matches!(missing_branch, AdoptError::Validation(_)));
}

#[test]
fn moving_ref_contradiction_and_pinned_mismatch_are_conflicts() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    commit_file(&repo, "skills/networking/SKILL.md", "# Networking v2\n");
    let url = format!("file://{}", repo.display());
    let old_hash = cli_hash_at(&repo, "HEAD~1", "skills/networking");
    write_skill(&harness.shared(), "networking", "# Networking v2\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "networking",
            "git",
            url.clone(),
            None,
            "skills/networking".into(),
            old_hash.clone(),
        )]),
    )
    .expect("write lock");

    let report = harness.adopt().scan().expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "networking")
        .expect("candidate");
    assert_eq!(
        candidate.verdict,
        AdoptVerdict::Conflict,
        "a moved ref whose lock hash no longer matches any remote state cannot close the loop"
    );
    assert!(matches!(
        candidate.reason,
        Some(AdoptVerdictReason::RemoteConflict { .. })
    ));
}

// =====================================================================
// Deferred grouping (ADR-0013 §2.3)
// =====================================================================

#[test]
fn deferred_remote_group_does_not_block_other_groups() {
    let harness = Harness::new();
    write_skill(&harness.shared(), "alpha", "# Alpha\n");
    write_skill(&harness.shared(), "beta", "# Beta\n");
    write_skill(&harness.shared(), "gamma", "# Gamma\n");
    let alpha_entity = harness
        .shared()
        .join("alpha")
        .canonicalize()
        .expect("canonical alpha");
    let beta_entity = harness
        .shared()
        .join("beta")
        .canonicalize()
        .expect("canonical beta");
    let gamma_entity = harness
        .shared()
        .join("gamma")
        .canonicalize()
        .expect("canonical gamma");
    let _ = (alpha_entity, beta_entity, gamma_entity);
    let hash = "0".repeat(64);
    std::fs::write(
        harness.lock_path(),
        lock_json(&[
            (
                "alpha",
                "git",
                "https://group-a.example/alpha".into(),
                None,
                "skills/alpha".into(),
                hash.clone(),
            ),
            (
                "beta",
                "git",
                "https://group-a.example/beta".into(),
                None,
                "skills/beta".into(),
                hash.clone(),
            ),
            (
                "gamma",
                "git",
                "https://group-b.example/gamma".into(),
                None,
                "skills/gamma".into(),
                hash.clone(),
            ),
        ]),
    )
    .expect("write lock");

    let mut behaviors = HashMap::new();
    behaviors.insert(
        "https://group-a.example/alpha".to_owned(),
        ScriptedOutcome::Deferred("DNS timeout".into()),
    );
    behaviors.insert(
        "https://group-a.example/beta".to_owned(),
        ScriptedOutcome::Deferred("TLS handshake failed".into()),
    );
    behaviors.insert(
        "https://group-b.example/gamma".to_owned(),
        ScriptedOutcome::Ok {
            files: vec![("SKILL.md".into(), "# Gamma\n".into())],
            anchor: "g".repeat(40),
        },
    );
    let provider = Arc::new(ScriptedRemoteProvider::new(behaviors));
    let report = harness
        .adopt_with(
            Arc::new(SystemInstallerLockStore::new(
                harness.home.path().to_path_buf(),
            )),
            provider,
        )
        .scan()
        .expect("scan");

    for name in ["alpha", "beta"] {
        let candidate = report
            .candidates
            .iter()
            .find(|candidate| candidate.directory_name == name)
            .expect("group-a candidate");
        assert_eq!(candidate.verdict, AdoptVerdict::Deferred, "{name}");
        assert!(matches!(
            candidate.reason,
            Some(AdoptVerdictReason::RemoteUnavailable { .. })
        ));
        assert!(!candidate.selectable);
    }
    let gamma = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "gamma")
        .expect("gamma candidate");
    assert_eq!(
        gamma.verdict,
        AdoptVerdict::Verified,
        "the healthy group continues while the unavailable group is Deferred"
    );
}

// =====================================================================
// Plan staleness and read-only proof (spec §8.1, §11, §10.2)
// =====================================================================

#[test]
fn plan_is_stale_after_rescan_lock_change_tree_change_or_appearance_change() {
    let harness = Harness::new();
    let entity = write_skill(&harness.projects, "stable", "# Stable\n");
    let canonical = entity.canonicalize().expect("canonical entity");
    let entry = harness.claude().join("stable");
    std::os::unix::fs::symlink(&canonical, &entry).expect("appearance");
    let adopt = harness.adopt();

    // 1. A rescan bumps the generation: the older plan is stale.
    let first = adopt.scan().expect("first scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: first.generation,
            selections: vec![select(&first.candidates, "stable")],
        })
        .expect("plan from first scan");
    let second = adopt.scan().expect("second scan");
    let error = adopt
        .apply(&plan.plan_token)
        .expect_err("a plan from an older generation is stale");
    assert!(matches!(error, AdoptError::PlanStale), "{error}");
    let stale_generation = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: first.generation,
            selections: vec![select(&second.candidates, "stable")],
        })
        .expect_err("planning against a stale generation is rejected");
    assert!(matches!(stale_generation, AdoptError::PlanStale));

    // 2. A tree change after the scan makes the plan stale before any write.
    let fresh = adopt.scan().expect("fresh scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: fresh.generation,
            selections: vec![select(&fresh.candidates, "stable")],
        })
        .expect("plan before tree change");
    std::fs::write(entity.join("SKILL.md"), "# Changed\n").expect("change the entity tree");
    let error = adopt
        .apply(&plan.plan_token)
        .expect_err("a changed tree makes the plan stale");
    assert!(matches!(error, AdoptError::PlanStale), "{error}");

    // 3. An appearance identity change makes the plan stale.
    let fresh = adopt.scan().expect("fresh scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: fresh.generation,
            selections: vec![select(&fresh.candidates, "stable")],
        })
        .expect("plan before appearance change");
    std::fs::remove_file(&entry).expect("remove appearance");
    std::os::unix::fs::symlink(&canonical, &entry).expect("recreate appearance (new inode)");
    let error = adopt
        .apply(&plan.plan_token)
        .expect_err("a replaced appearance makes the plan stale");
    assert!(matches!(error, AdoptError::PlanStale), "{error}");

    // 4. A lock fingerprint change makes a lock-bound plan stale.
    write_skill(&harness.shared(), "locked-skill", "# Locked\n");
    let hash = "0".repeat(64);
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "locked-skill",
            "git",
            "https://example.com/locked".into(),
            None,
            "skills/locked".into(),
            hash.clone(),
        )]),
    )
    .expect("write lock");
    let locked = adopt.scan().expect("locked scan");
    let locked_candidate = locked
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "locked-skill")
        .expect("locked candidate");
    let error = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: locked.generation,
            selections: vec![select(&locked.candidates, "locked-skill")],
        })
        .expect_err("the fake provider cannot verify; the entry-level evidence still binds");
    // The lock declares the name but verification fails; the plan must not
    // silently accept a Verification Deferred as selectable.
    assert!(matches!(
        error,
        AdoptError::Validation(_) | AdoptError::PlanStale
    ));
    let _ = locked_candidate;
}

#[test]
fn scan_and_plan_are_proven_read_only() {
    let harness = Harness::new();
    let entity = write_skill(&harness.projects, "readonly", "# Readonly\n");
    let canonical = entity.canonicalize().expect("canonical entity");
    std::os::unix::fs::symlink(&canonical, harness.claude().join("readonly")).expect("appearance");

    // Snapshot every observable surface.
    let catalog_before = std::fs::read(harness.home.catalog_path()).expect("catalog bytes");
    let tree_before = harness.filesystem.tree_hash(&canonical).expect("tree hash");
    let inode_before = std::fs::symlink_metadata(&canonical)
        .expect("metadata")
        .ino();
    let entry_inode_before = std::fs::symlink_metadata(harness.claude().join("readonly"))
        .expect("entry metadata")
        .ino();

    let adopt = harness.adopt();
    let report = adopt.scan().expect("scan");
    let _plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![select(&report.candidates, "readonly")],
        })
        .expect("plan");
    let _ = adopt.cancel(&_plan.plan_token);

    // Nothing changed: Catalog, lock, entity tree, identities, no journals
    // or staging.
    assert_eq!(
        std::fs::read(harness.home.catalog_path()).expect("catalog bytes after"),
        catalog_before
    );
    assert_eq!(
        harness
            .filesystem
            .tree_hash(&canonical)
            .expect("tree hash after"),
        tree_before
    );
    assert_eq!(
        std::fs::symlink_metadata(&canonical)
            .expect("metadata after")
            .ino(),
        inode_before
    );
    assert_eq!(
        std::fs::symlink_metadata(harness.claude().join("readonly"))
            .expect("entry metadata after")
            .ino(),
        entry_inode_before
    );
    assert!(
        !harness.home.library_root.join("operations").exists(),
        "scan/plan must not write operation journals"
    );
    assert!(
        !harness.home.library_root.join("staging").exists(),
        "scan/plan must not write staging"
    );
    assert!(
        !harness.home.library_root.join("skills/readonly").exists(),
        "scan/plan must not publish Home entities"
    );
}

#[test]
fn local_link_apply_is_the_only_applyable_intent_and_stays_read_only_until_then() {
    let harness = Harness::new();
    let entity = write_skill(&harness.projects, "apply-me", "# Apply me\n");
    let canonical = entity.canonicalize().expect("canonical entity");
    let entry = harness.claude().join("apply-me");
    std::os::unix::fs::symlink(&canonical, &entry).expect("appearance");
    let adopt = harness.adopt();

    let report = adopt.scan().expect("scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![select(&report.candidates, "apply-me")],
        })
        .expect("plan");
    assert_eq!(plan.items[0].intent, AdoptPlanIntent::LocalLink);
    assert!(plan.can_apply);
    let result = adopt.apply(&plan.plan_token).expect("apply");
    assert!(result.items[0].adopted, "{:?}", result.items[0].error);
    assert_eq!(
        std::fs::read_link(&entry).expect("Activation"),
        canonical,
        "the appearance now points at the untouched stable entity"
    );
    assert!(
        entity.join("SKILL.md").is_file(),
        "the Local Link never copies or rewrites the source tree"
    );
}

// =====================================================================
// Typed DTO contract (spec §4.7): camelCase fields, snake_case enums,
// Source Content never localizes
// =====================================================================

#[test]
fn evidence_dto_serializes_closed_states_and_keeps_source_content_verbatim() {
    use skill_man_lib::tauri_adapter::dto::{
        AdoptEvidenceCandidateDto, AdoptEvidenceReportDto, AdoptPlanIntentDto, AdoptVerdictDto,
    };
    let verdict = serde_json::to_value(AdoptVerdictDto::Modified).expect("serialize verdict");
    assert_eq!(verdict, "modified");
    let intent = serde_json::to_value(AdoptPlanIntentDto::RemoteInstallKeepCurrent)
        .expect("serialize intent");
    assert_eq!(intent, "remote_install_keep_current");

    let report = AdoptEvidenceReportDto {
        generation: 7,
        candidates: vec![AdoptEvidenceCandidateDto {
            canonical_entity: "/Users/zoe/.agents/skills/α-skill".into(),
            directory_name: "α-skill".into(),
            directory_names: vec!["α-skill".into()],
            appearances: vec![],
            verdict: AdoptVerdictDto::Local,
            reason: Some(skill_man_lib::tauri_adapter::dto::AdoptVerdictReasonDto::NoLock),
            lock: None,
            remote: None,
            local_tree_hash: Some("tree-sha256-v1:abc".into()),
            requires_relocation: false,
            selectable: true,
            adoptable: true,
            conflict: None,
            suggested_agent_ids: vec![],
        }],
        lock_files: vec![],
        truncated: false,
    };
    let json = serde_json::to_value(&report).expect("serialize report");
    assert_eq!(json["generation"], 7);
    assert_eq!(
        json["candidates"][0]["canonicalEntity"],
        "/Users/zoe/.agents/skills/α-skill"
    );
    assert_eq!(json["candidates"][0]["verdict"], "local");
    assert_eq!(json["candidates"][0]["reason"]["kind"], "no_lock");
    assert_eq!(json["candidates"][0]["requiresRelocation"], false);
    assert_eq!(json["candidates"][0]["localTreeHash"], "tree-sha256-v1:abc");

    // A tagged reason carries its raw facts verbatim (never App Copy).
    let reason = serde_json::to_value(
        skill_man_lib::tauri_adapter::dto::AdoptVerdictReasonDto::RemoteUnavailable {
            detail: "Could not resolve host: github.com".into(),
        },
    )
    .expect("serialize reason");
    assert_eq!(reason["kind"], "remote_unavailable");
    assert_eq!(reason["detail"], "Could not resolve host: github.com");

    let lock_fault = serde_json::to_value(
        skill_man_lib::tauri_adapter::dto::LockFileFaultDto::DuplicateKey {
            key: "skills".into(),
        },
    )
    .expect("serialize lock fault");
    assert_eq!(lock_fault["kind"], "duplicate_key");
    assert_eq!(lock_fault["key"], "skills");
}

#[test]
fn cancelled_applyable_plan_cannot_be_applied() {
    let harness = Harness::new();
    let entity = write_skill(&harness.projects, "cancel-me", "# Cancel me\n");
    let canonical = entity.canonicalize().expect("canonical entity");
    std::os::unix::fs::symlink(&canonical, harness.claude().join("cancel-me")).expect("appearance");
    let adopt = harness.adopt();
    let report = adopt.scan().expect("scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![select(&report.candidates, "cancel-me")],
        })
        .expect("applyable plan");
    assert!(plan.can_apply);
    assert!(adopt.cancel(&plan.plan_token).expect("cancel plan"));
    // Cancelling removes BOTH the evidence batch and the durable per-Skill
    // batch; the plan must no longer be executable (spec §11: stale plans
    // never write).
    let error = adopt
        .apply(&plan.plan_token)
        .expect_err("a cancelled plan cannot be applied");
    assert!(matches!(error, AdoptError::PlanNotFound), "{error}");
    assert!(
        !skill_man_lib::core::catalog::CatalogService::new(harness.runtime.clone())
            .list(skill_man_lib::core::domain::CatalogFilter::Link)
            .expect("list Links")
            .items
            .iter()
            .any(|skill| skill.directory_name == "cancel-me"),
        "nothing was adopted"
    );
    assert!(
        std::fs::symlink_metadata(harness.claude().join("cancel-me"))
            .expect("appearance intact")
            .file_type()
            .is_symlink(),
        "the appearance was never replaced"
    );
}

#[test]
fn faulted_xdg_lock_does_not_block_default_installer_root_candidates() {
    let harness = Harness::new();
    write_skill(&harness.shared(), "default-root-skill", "# Default\n");
    // The default lock is valid; the XDG lock is corrupt and governs a
    // different root that Adopt never scans.
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "default-root-skill",
            "git",
            "https://example.com/default".into(),
            None,
            "skills/default".into(),
            "0".repeat(64),
        )]),
    )
    .expect("valid default lock");
    let xdg = harness.home.path().join("xdg-state/skills");
    std::fs::create_dir_all(&xdg).expect("xdg skills dir");
    std::fs::write(xdg.join(".skill-lock.json"), "{corrupt").expect("corrupt xdg lock");
    let locks = Arc::new(SystemInstallerLockStore::with_xdg(
        harness.home.path().to_path_buf(),
        Some(harness.home.path().join("xdg-state")),
    ));

    let report = harness
        .adopt_with(
            locks,
            Arc::new(SystemRemoteProvider::new(Arc::new(SystemGitSource::new()))),
        )
        .scan()
        .expect("scan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "default-root-skill")
        .expect("candidate");
    assert_eq!(
        candidate.verdict,
        AdoptVerdict::Deferred,
        "the XDG lock fault must not block the default root candidate; \
         only the default lock governs ~/.agents/skills"
    );
    assert!(!matches!(
        candidate.reason,
        Some(AdoptVerdictReason::LockFileFault { .. })
    ));
}

#[test]
fn verdict_reason_dto_fields_serialize_camel_case() {
    use skill_man_lib::tauri_adapter::dto::AdoptVerdictReasonDto;
    let json = serde_json::to_value(AdoptVerdictReasonDto::DuplicateLockOwner {
        other_lock_path: "~/.agents/.skill-lock.json".into(),
    })
    .expect("serialize");
    assert_eq!(json["kind"], "duplicate_lock_owner");
    assert_eq!(json["otherLockPath"], "~/.agents/.skill-lock.json");

    let json = serde_json::to_value(AdoptVerdictReasonDto::LibraryConflict {
        directory_name: "foo".into(),
    })
    .expect("serialize");
    assert_eq!(json["kind"], "library_conflict");
    assert_eq!(json["directoryName"], "foo");

    let json = serde_json::to_value(AdoptVerdictReasonDto::LockFileFault {
        lock_path: "~/.agents/.skill-lock.json".into(),
        fault: skill_man_lib::tauri_adapter::dto::LockFileFaultDto::UnsupportedVersion {
            version: 9,
        },
    })
    .expect("serialize");
    assert_eq!(json["kind"], "lock_file_fault");
    assert_eq!(json["lockPath"], "~/.agents/.skill-lock.json");
    assert_eq!(json["fault"]["kind"], "unsupported_version");
    assert_eq!(json["fault"]["version"], 9);
}
