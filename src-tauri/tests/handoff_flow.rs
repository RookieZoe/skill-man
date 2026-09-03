//! Ownership Handoff vertical slice (spec §8.4, ADR-0013 §5): real
//! temp-HOME + real Git + real SQLite tests for the Planned → Staged →
//! Source Isolated → External Ownership Released (lock-entry CAS) →
//! Managed Committed → Finalized state machine, the exact-entry CAS lock
//! writer, pre-CAS rollback, post-CAS roll-forward under the recovery gate,
//! conditional Undo guards, last-child Remove and Ownership Conflict.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::remote_provider::SystemRemoteProvider;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::adopt::{
    AdoptPlanIntent, AdoptPlanRequest, AdoptSelection, AdoptService, AdoptVerdict,
    AdoptVerdictReason, ModifiedBranch,
};
use skill_man_lib::core::maintenance::MaintenanceService;
use skill_man_lib::core::update::{UpdateSelection, UpdateService};
use skill_man_lib::seams::filesystem::{FileSystem, RemoteParentManifest};
use skill_man_lib::seams::import_store::ImportStore;
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

/// (name, sourceType, sourceUrl, ref, skillPath, hash)
type LockRow<'a> = (&'a str, &'a str, String, Option<&'a str>, String, String);

fn lock_json(entries: &[LockRow<'_>]) -> String {
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
            format!(r#""{name}": {{{}}}"#, fields.join(",\n"))
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!(r#"{{"version": 3, "skills": {{{rendered}}}}}"#)
}

struct Harness {
    home: BoundTestHome,
    projects: PathBuf,
    filesystem: Arc<MacOsFileSystem>,
    runtime: Arc<RuntimeCatalogStore>,
    write_gate: Arc<skill_man_lib::core::write_gate::WriteGate>,
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
        let write_gate = home.write_gate.clone();
        Self {
            home,
            projects,
            filesystem,
            runtime,
            write_gate,
        }
    }

    fn shared(&self) -> PathBuf {
        self.home.path().join(".agents/skills")
    }

    fn lock_path(&self) -> PathBuf {
        self.home.path().join(".agents/.skill-lock.json")
    }

    fn adopt(&self) -> AdoptService {
        AdoptService::new(
            self.runtime.clone(),
            self.filesystem.clone(),
            Arc::new(SystemClock::new()),
            self.home.library_root.clone(),
            self.home.path().to_path_buf(),
        )
        .with_write_gate(self.write_gate.clone())
        .with_lock_store(Arc::new(SystemInstallerLockStore::new(
            self.home.path().to_path_buf(),
        )))
        .with_remote_provider(Arc::new(SystemRemoteProvider::new(Arc::new(
            SystemGitSource::new(),
        ))))
    }

    fn maintenance(&self) -> MaintenanceService {
        MaintenanceService::new(self.runtime.clone(), self.filesystem.clone())
            .with_library_root(self.home.library_root.clone())
            .with_write_gate(self.write_gate.clone())
    }

    fn update(&self) -> UpdateService {
        let git_cache_root = self.home.library_root.join("cache");
        UpdateService::new(
            self.runtime.clone(),
            self.filesystem.clone(),
            Arc::new(SystemClock::new()),
            Arc::new(SystemGitSource::new()),
            self.home.library_root.clone(),
            git_cache_root,
            self.write_gate.clone(),
        )
    }

    fn select(
        &self,
        candidates: &[skill_man_lib::core::adopt::AdoptEvidenceCandidate],
        name: &str,
        modified_branch: Option<ModifiedBranch>,
        target_directory: Option<PathBuf>,
    ) -> AdoptSelection {
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.directory_name == name)
            .unwrap_or_else(|| panic!("candidate '{name}' not found"));
        AdoptSelection {
            canonical_entity: candidate.canonical_entity.clone(),
            agent_ids: Vec::new(),
            modified_branch,
            target_directory,
        }
    }

    fn tree_hash(&self, path: &Path) -> String {
        self.filesystem.tree_hash(path).expect("tree hash")
    }
}

/// The lock file bytes with a fixed extra top-level field, to prove the CAS
/// rewrite preserves unknown JSON.
fn lock_json_with_extra(entries: &[LockRow<'_>]) -> String {
    let mut value: serde_json::Value =
        serde_json::from_str(&lock_json(entries)).expect("lock JSON");
    value
        .as_object_mut()
        .expect("lock object")
        .insert("installerVersion".into(), serde_json::json!("9.9.9"));
    value.as_object_mut().expect("lock object").insert(
        "installed".into(),
        serde_json::json!([{"name": "other-skill", "at": "2026-01-01T00:00:00Z"}]),
    );
    serde_json::to_string_pretty(&value).expect("pretty lock")
}

// =====================================================================
// Verified Handoff end-to-end (KeepCurrent)
// =====================================================================

#[test]
fn verified_handoff_keep_current_commits_home_entity_and_releases_the_lock() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
    let local = write_skill(&harness.shared(), "networking", "# Networking v1\n");
    let local_canonical = local.canonicalize().expect("canonical entity");
    let local_hash = harness.tree_hash(&local_canonical);
    std::fs::write(
        harness.lock_path(),
        lock_json_with_extra(&[(
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
    if candidate.verdict != AdoptVerdict::Verified {
        eprintln!(
            "DBG verdict={:?} reason={:?} selectable={} adoptable={} requires={}",
            candidate.verdict,
            candidate.reason,
            candidate.selectable,
            candidate.adoptable,
            candidate.requires_relocation
        );
    }
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "networking", None, None)],
        })
        .expect("plan");
    assert_eq!(
        plan.items[0].intent,
        AdoptPlanIntent::RemoteInstallKeepCurrent
    );
    assert!(plan.can_apply);

    let result = adopt.apply(&plan.plan_token).expect("apply handoff");
    assert_eq!(result.items.len(), 1);
    assert!(result.items[0].adopted, "{:?}", result.items[0].error);
    assert!(result.undo_available);

    // Home tree equals the user's pre-apply tree, byte for byte.
    let home_entity = harness.home.library_root.join("skills/networking");
    assert!(home_entity.join("SKILL.md").is_file());
    assert_eq!(
        std::fs::read_to_string(home_entity.join("SKILL.md")).expect("Home SKILL.md"),
        "# Networking v1\n"
    );
    assert_eq!(harness.tree_hash(&home_entity), local_hash);

    // The external canonical directory is gone; the lock entry was
    // CAS-removed while every other value survived.
    assert!(!local_canonical.exists(), "external canonical dir removed");
    let lock_bytes = std::fs::read(harness.lock_path()).expect("lock remains");
    let lock: serde_json::Value = serde_json::from_slice(&lock_bytes).expect("lock JSON");
    assert_eq!(lock["version"], 3);
    assert_eq!(lock["installerVersion"], "9.9.9", "unknown field preserved");
    assert_eq!(
        lock["installed"][0]["name"], "other-skill",
        "unknown array preserved"
    );
    assert!(
        lock["skills"].as_object().expect("skills").is_empty(),
        "the last entry removal keeps a valid empty v3 lock"
    );

    // Parent manifest + row + binding are consistent.
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].directory_name, "networking");
    assert_eq!(records[0].source_url, url);
    assert_eq!(records[0].requested_ref, "HEAD");
    assert_eq!(records[0].verification_anchor_commit, head_of(&repo));
    assert!(
        !records[0].original_commit_known,
        "a moving ref anchor never claims the unrecorded original install commit"
    );
    assert_eq!(records[0].skill_path, "skills/networking");
    assert_eq!(records[0].provider_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(
        records[0].remote_baseline_hash,
        records[0].current_baseline_hash
    );
    assert_eq!(
        records[0].health,
        skill_man_lib::core::domain::Health::Healthy
    );
    let parent = harness
        .runtime
        .find_remote_parent_by_url(&url)
        .expect("parent")
        .expect("parent exists");
    let manifest = harness
        .filesystem
        .read_remote_parent_manifest(
            &harness.home.library_root.join("remotes"),
            &parent.remote_id,
        )
        .expect("read manifest")
        .expect("manifest exists");
    assert_eq!(manifest.remote_id, parent.remote_id);
    assert_eq!(manifest.canonical_url, url);

    // The real Agent appearances were flattened into Activations pointing
    // at the Home entity; the shared/installer root has none.
    let claude_entry = harness.home.claude_root().join("networking");
    assert!(claude_entry.is_symlink());
    let target = std::fs::read_link(&claude_entry).expect("read link");
    assert_eq!(target, home_entity);
    assert!(!harness.shared().join("networking").exists());

    // Conditional Undo restores the exact entry, entity and appearances.
    let undo = adopt.undo(&result.operation_id).expect("undo");
    assert_eq!(undo.items.len(), 1);
    assert!(undo.items[0].undone);
    assert!(local_canonical.exists(), "external entity restored");
    assert_eq!(
        std::fs::read_to_string(local_canonical.join("SKILL.md")).expect("restored SKILL.md"),
        "# Networking v1\n"
    );
    assert!(!home_entity.exists(), "Home entity removed");
    assert!(!claude_entry.exists(), "Activation removed");
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.lock_path()).expect("lock")).expect("JSON");
    assert_eq!(
        lock["skills"]["networking"]["skillFolderHash"], hash,
        "entry restored"
    );
    assert_eq!(
        lock["installerVersion"], "9.9.9",
        "unknown fields survive Undo"
    );
    let records = harness.runtime.load_remote_installs().expect("records");
    assert!(records.is_empty(), "binding removed");
    assert!(
        harness
            .runtime
            .find_remote_parent_by_url(&url)
            .expect("parent lookup")
            .is_none(),
        "last-child parent removed"
    );
}

fn head_of(repo: &Path) -> String {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .expect("rev-parse");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

// =====================================================================
// Modified three-way branches
// =====================================================================

fn modified_candidate(repo: &Path, harness: &Harness) -> AdoptSelection {
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(repo, "HEAD", "skills/networking");
    write_skill(
        &harness.shared(),
        "networking",
        "# Networking v1\nlocal change\n",
    );
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
    harness.select(&report.candidates, "networking", None, None)
}

#[test]
fn modified_keep_current_preserves_local_bytes_and_marks_modified() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let selection = modified_candidate(&repo, &harness);
    let adopt = harness.adopt();
    let report = adopt.scan().expect("rescan");
    let selection = AdoptSelection {
        modified_branch: Some(ModifiedBranch::KeepCurrent),
        ..selection
    };
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![selection],
        })
        .expect("plan");
    assert_eq!(
        plan.items[0].intent,
        AdoptPlanIntent::RemoteInstallKeepCurrent
    );
    let result = adopt.apply(&plan.plan_token).expect("apply");
    assert!(result.items[0].adopted);
    let home_entity = harness.home.library_root.join("skills/networking");
    assert_eq!(
        std::fs::read_to_string(home_entity.join("SKILL.md")).expect("SKILL.md"),
        "# Networking v1\nlocal change\n",
        "KeepCurrent preserves the local bytes"
    );
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(
        records[0].health,
        skill_man_lib::core::domain::Health::Modified
    );
    assert_ne!(
        records[0].remote_baseline_hash,
        records[0].current_baseline_hash
    );
    // The remote baseline is the anchor tree, not the modified local tree.
    let anchor = head_of(&repo);
    assert_eq!(records[0].verification_anchor_commit, anchor);
}

#[test]
fn modified_discard_to_anchor_installs_the_anchor_tree_not_the_tip() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let anchor = head_of(&repo);
    let selection = modified_candidate(&repo, &harness);
    let adopt = harness.adopt();
    let report = adopt.scan().expect("rescan");
    let selection = AdoptSelection {
        modified_branch: Some(ModifiedBranch::DiscardToAnchor),
        ..selection
    };
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![selection],
        })
        .expect("plan");
    assert_eq!(
        plan.items[0].intent,
        AdoptPlanIntent::RemoteInstallDiscardModified
    );
    // Upstream moves after the plan froze; the anchor stays v1.
    commit_file(&repo, "skills/networking/SKILL.md", "# Networking v2\n");
    let result = adopt.apply(&plan.plan_token).expect("apply");
    assert!(result.items[0].adopted);
    let home_entity = harness.home.library_root.join("skills/networking");
    assert_eq!(
        std::fs::read_to_string(home_entity.join("SKILL.md")).expect("SKILL.md"),
        "# Networking v1\n",
        "DiscardToAnchor materializes the Verification Anchor, never the ref tip"
    );
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records[0].verification_anchor_commit, anchor);
    assert_eq!(
        records[0].health,
        skill_man_lib::core::domain::Health::Healthy
    );
}

#[test]
fn modified_convert_to_link_moves_to_the_chosen_directory_without_a_binding() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let selection = modified_candidate(&repo, &harness);
    let target = harness.projects.join("stable-skills/networking");
    let adopt = harness.adopt();
    let report = adopt.scan().expect("rescan");
    let selection = AdoptSelection {
        modified_branch: Some(ModifiedBranch::ConvertToLocalLink),
        target_directory: Some(target.clone()),
        ..selection
    };
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![selection],
        })
        .expect("plan");
    assert_eq!(
        plan.items[0].intent,
        AdoptPlanIntent::RemoteInstallConvertToLink
    );
    assert!(plan.can_apply);
    let result = adopt.apply(&plan.plan_token).expect("apply");
    assert!(result.items[0].adopted);
    assert_eq!(
        std::fs::read_to_string(target.join("SKILL.md")).expect("SKILL.md"),
        "# Networking v1\nlocal change\n",
        "the current bytes move to the user-chosen stable directory"
    );
    // Link registration: no binding, no parent, lock entry gone.
    assert!(
        harness
            .runtime
            .load_remote_installs()
            .expect("records")
            .is_empty()
    );
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.lock_path()).expect("lock")).expect("JSON");
    assert!(lock["skills"].as_object().expect("skills").is_empty());
    // Undo moves the tree back and restores the entry.
    let undo = adopt.undo(&result.operation_id).expect("undo");
    assert!(undo.items[0].undone);
    assert!(
        harness
            .shared()
            .join("networking")
            .join("SKILL.md")
            .is_file()
    );
    assert!(!target.exists());
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.lock_path()).expect("lock")).expect("JSON");
    assert!(lock["skills"]["networking"].is_object(), "entry restored");
}

// =====================================================================
// Same remote multi-Skill, per-Skill independence, CAS refusal
// =====================================================================

#[test]
fn same_remote_multiple_skills_share_one_parent_and_independent_bindings() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[
            ("skills/networking/SKILL.md", "# Networking v1\n"),
            ("skills/audio/SKILL.md", "# Audio v1\n"),
        ],
    );
    let url = format!("file://{}", repo.display());
    let hash_net = cli_hash_at(&repo, "HEAD", "skills/networking");
    let hash_audio = cli_hash_at(&repo, "HEAD", "skills/audio");
    write_skill(&harness.shared(), "networking", "# Networking v1\n");
    write_skill(&harness.shared(), "audio", "# Audio v1\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[
            (
                "networking",
                "git",
                url.clone(),
                None,
                "skills/networking".into(),
                hash_net.clone(),
            ),
            (
                "audio",
                "git",
                url.clone(),
                None,
                "skills/audio".into(),
                hash_audio.clone(),
            ),
        ]),
    )
    .expect("write lock");

    let adopt = harness.adopt();
    let report = adopt.scan().expect("scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![
                harness.select(&report.candidates, "networking", None, None),
                harness.select(&report.candidates, "audio", None, None),
            ],
        })
        .expect("plan");
    assert_eq!(plan.items.len(), 2);

    let result = adopt.apply(&plan.plan_token).expect("apply batch");
    assert_eq!(result.items.len(), 2);
    assert!(result.items[0].adopted, "{:?}", result.items[0].error);
    assert!(result.items[1].adopted, "{:?}", result.items[1].error);

    // One parent, two bindings, each with its own anchor/baseline.
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records.len(), 2);
    let parents = harness.runtime.load_remote_parents().expect("parents");
    assert_eq!(parents.len(), 1, "one parent per repository");
    assert!(
        records
            .iter()
            .all(|record| record.remote_id == parents[0].remote_id)
    );
    assert_ne!(records[0].skill_id, records[1].skill_id);
    let by_name = |name: &str| {
        records
            .iter()
            .find(|record| record.directory_name == name)
            .expect("record")
    };
    assert_eq!(by_name("networking").skill_path, "skills/networking");
    assert_eq!(by_name("audio").skill_path, "skills/audio");
    for record in &records {
        assert_eq!(record.verification_anchor_commit, head_of(&repo));
    }
    assert!(
        harness
            .home
            .library_root
            .join("skills/networking")
            .join("SKILL.md")
            .is_file()
    );
    assert!(
        harness
            .home
            .library_root
            .join("skills/audio")
            .join("SKILL.md")
            .is_file()
    );
}

#[test]
fn lock_change_before_apply_is_plan_stale_with_zero_partial_commits() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[
            ("skills/networking/SKILL.md", "# Networking v1\n"),
            ("skills/audio/SKILL.md", "# Audio v1\n"),
        ],
    );
    let url = format!("file://{}", repo.display());
    let hash_net = cli_hash_at(&repo, "HEAD", "skills/networking");
    let hash_audio = cli_hash_at(&repo, "HEAD", "skills/audio");
    write_skill(&harness.shared(), "networking", "# Networking v1\n");
    write_skill(&harness.shared(), "audio", "# Audio v1\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[
            (
                "networking",
                "git",
                url.clone(),
                None,
                "skills/networking".into(),
                hash_net.clone(),
            ),
            (
                "audio",
                "git",
                url.clone(),
                None,
                "skills/audio".into(),
                hash_audio.clone(),
            ),
        ]),
    )
    .expect("write lock");

    let adopt = harness.adopt();
    let report = adopt.scan().expect("scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![
                harness.select(&report.candidates, "networking", None, None),
                harness.select(&report.candidates, "audio", None, None),
            ],
        })
        .expect("plan");

    // The installer rewrites the lock after the plan froze: the TOCTOU
    // recheck at Apply must refuse with PlanStale and zero partial writes.
    std::fs::write(
        harness.lock_path(),
        lock_json(&[
            (
                "networking",
                "git",
                url.clone(),
                None,
                "skills/networking".into(),
                hash_net.clone(),
            ),
            (
                "audio",
                "git",
                url.clone(),
                None,
                "skills/audio".into(),
                hash_audio.clone(),
            ),
            (
                "newbie",
                "git",
                url.clone(),
                None,
                "skills/newbie".into(),
                hash_audio.clone(),
            ),
        ]),
    )
    .expect("rewrite lock");

    let error = adopt
        .apply(&plan.plan_token)
        .expect_err("stale plan refused");
    assert!(
        matches!(error, skill_man_lib::core::adopt::AdoptError::PlanStale),
        "{error}"
    );
    // Zero partial commits: no Home entity, no binding, external sources
    // untouched, lock untouched.
    assert!(
        harness
            .runtime
            .load_remote_installs()
            .expect("records")
            .is_empty()
    );
    assert!(
        harness
            .shared()
            .join("networking")
            .join("SKILL.md")
            .is_file()
    );
    assert!(harness.shared().join("audio").join("SKILL.md").is_file());
    assert!(!harness.home.library_root.join("skills/networking").exists());
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.lock_path()).expect("lock")).expect("JSON");
    assert!(lock["skills"]["networking"].is_object());
    assert!(lock["skills"]["audio"].is_object());
}

/// A lock store that CAS-refuses the release of one exact entry, exactly
/// like a concurrent installer rewrite between the apply recheck and the
/// commit point.
struct RefuseEntryLockStore {
    delegate: SystemInstallerLockStore,
    refused_name: String,
}

impl skill_man_lib::seams::installer_lock_store::InstallerLockStore for RefuseEntryLockStore {
    fn discover(
        &self,
    ) -> Result<
        Vec<skill_man_lib::seams::installer_lock_store::LockFileReport>,
        skill_man_lib::seams::installer_lock_store::InstallerLockError,
    > {
        self.delegate.discover()
    }

    fn release_entry(
        &self,
        lock_path: &std::path::Path,
        frozen_fingerprint: &str,
        entry: &skill_man_lib::seams::installer_lock_store::LockEntry,
    ) -> Result<(), skill_man_lib::seams::installer_lock_store::LockReleaseError> {
        if entry.name == self.refused_name {
            return Err(
                skill_man_lib::seams::installer_lock_store::LockReleaseError::FingerprintChanged,
            );
        }
        self.delegate
            .release_entry(lock_path, frozen_fingerprint, entry)
    }

    fn restore_entry(
        &self,
        lock_path: &std::path::Path,
        entry: &skill_man_lib::seams::installer_lock_store::LockEntry,
    ) -> Result<(), skill_man_lib::seams::installer_lock_store::LockReleaseError> {
        self.delegate.restore_entry(lock_path, entry)
    }
}

#[test]
fn cas_refusal_restores_the_source_and_stops_uncommitted_items() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[
            ("skills/networking/SKILL.md", "# Networking v1\n"),
            ("skills/audio/SKILL.md", "# Audio v1\n"),
            ("skills/gamma/SKILL.md", "# Gamma v1\n"),
        ],
    );
    let url = format!("file://{}", repo.display());
    let hash = |path: &str| cli_hash_at(&repo, "HEAD", path);
    write_skill(&harness.shared(), "networking", "# Networking v1\n");
    write_skill(&harness.shared(), "audio", "# Audio v1\n");
    write_skill(&harness.shared(), "gamma", "# Gamma v1\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[
            (
                "networking",
                "git",
                url.clone(),
                None,
                "skills/networking".into(),
                hash("skills/networking"),
            ),
            (
                "audio",
                "git",
                url.clone(),
                None,
                "skills/audio".into(),
                hash("skills/audio"),
            ),
            (
                "gamma",
                "git",
                url.clone(),
                None,
                "skills/gamma".into(),
                hash("skills/gamma"),
            ),
        ]),
    )
    .expect("write lock");

    let adopt = AdoptService::new(
        harness.runtime.clone(),
        harness.filesystem.clone(),
        Arc::new(SystemClock::new()),
        harness.home.library_root.clone(),
        harness.home.path().to_path_buf(),
    )
    .with_write_gate(harness.write_gate.clone())
    .with_lock_store(Arc::new(RefuseEntryLockStore {
        delegate: SystemInstallerLockStore::new(harness.home.path().to_path_buf()),
        refused_name: "audio".into(),
    }))
    .with_remote_provider(Arc::new(SystemRemoteProvider::new(Arc::new(
        SystemGitSource::new(),
    ))));
    let report = adopt.scan().expect("scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![
                harness.select(&report.candidates, "networking", None, None),
                harness.select(&report.candidates, "audio", None, None),
                harness.select(&report.candidates, "gamma", None, None),
            ],
        })
        .expect("plan");

    let result = adopt.apply(&plan.plan_token).expect("apply batch");
    assert_eq!(result.items.len(), 3);
    assert!(result.items[0].adopted, "{:?}", result.items[0].error);
    assert!(!result.items[1].adopted);
    assert!(
        result.items[1]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("lock changed")),
        "{:?}",
        result.items[1].error
    );
    // The remaining uncommitted item is stopped without any write.
    assert!(!result.items[2].adopted);
    assert!(
        result.items[2]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not applied")),
        "{:?}",
        result.items[2].error
    );

    // The failed item's external source is restored in place; the
    // successful item stays; the skipped item's source is untouched.
    assert!(
        harness.shared().join("audio").join("SKILL.md").is_file(),
        "audio restored in place"
    );
    assert!(
        harness.shared().join("gamma").join("SKILL.md").is_file(),
        "gamma never touched"
    );
    assert!(
        !harness.shared().join("networking").exists(),
        "networking released"
    );
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].directory_name, "networking");
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.lock_path()).expect("lock")).expect("JSON");
    assert!(lock["skills"]["audio"].is_object(), "refused entry stays");
    assert!(lock["skills"]["gamma"].is_object(), "skipped entry stays");
    assert!(
        !lock["skills"]["networking"].is_object(),
        "released entry is gone"
    );
}

#[test]
fn parent_manifest_mismatch_closes_only_that_parents_update() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
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
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "networking", None, None)],
        })
        .expect("plan");
    adopt.apply(&plan.plan_token).expect("apply");

    // A foreign process rewrites the parent manifest to another URL: the
    // parent's Update closes, read/Remove stay open.
    let parent = harness
        .runtime
        .find_remote_parent_by_url(&url)
        .expect("parent lookup")
        .expect("parent");
    harness
        .filesystem
        .write_remote_parent_manifest(
            &harness.home.library_root.join("remotes"),
            &RemoteParentManifest {
                schema_version: 1,
                remote_id: parent.remote_id.clone(),
                canonical_url: "https://evil.example/other-repo".into(),
                provider: None,
                tracking_mode: None,
                tracking_value: None,
                current_selected_ref: None,
                current_release_id: None,
                aliases: Vec::new(),
                created_at: parent.created_at.clone(),
            },
        )
        .expect("rewrite manifest");

    // Update check reports the parent conflict and skips the group; the
    // skill stays readable and removable.
    let report = harness.update().check_updates(true).expect("check");
    assert_eq!(report.parent_conflicts.len(), 1);
    assert_eq!(report.parent_conflicts[0].remote_id, parent.remote_id);
    assert_eq!(report.parent_conflicts[0].canonical_url, url);
    assert!(report.groups.is_empty());
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records.len(), 1, "read keeps working");
    let remove_preview = harness
        .maintenance()
        .plan_remove(&records[0].skill_id)
        .expect("plan remove still works");
    assert_eq!(remove_preview.directory_name, "networking");
}

// =====================================================================
// Crash matrix: pre-CAS rollback and post-CAS roll-forward
// =====================================================================

#[test]
fn crash_before_cas_rolls_back_in_place_on_startup() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
    let local = write_skill(&harness.shared(), "networking", "# Networking v1\n");
    let local_canonical = local.canonicalize().expect("canonical");
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
    // The plan validates the frozen world; the crash emulation below starts
    // from the journal state the apply would have left behind.
    let _plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "networking", None, None)],
        })
        .expect("plan");

    // Emulate a crash right after the source isolation
    // (before the CAS): simulate by running apply and killing the process
    // is not feasible here, so emulate the crash by writing the journal
    // state the apply would have left: isolate the source by hand and set
    // the journal to SourceIsolated. The lock is untouched.
    let operation_root = harness
        .home
        .library_root
        .join("operations")
        .join("handoff-1");
    std::fs::create_dir_all(&operation_root).expect("operation dir");
    let staged_root = harness
        .home
        .library_root
        .join("staging")
        .join("handoff-1")
        .join("networking");
    std::fs::create_dir_all(staged_root.parent().expect("staging parent")).expect("staging root");
    std::fs::create_dir_all(&staged_root).expect("staged directory");
    std::fs::write(staged_root.join("SKILL.md"), "# Networking v1\n").expect("staged copy");
    let isolated = harness
        .shared()
        .join(".skill-man-handoff-handoff-1-networking");
    std::fs::rename(&local_canonical, &isolated).expect("isolate source");
    let entry = skill_man_lib::seams::installer_lock_store::LockEntry {
        name: "networking".into(),
        source_type: "git".into(),
        source: url.clone(),
        source_url: url.clone(),
        requested_ref: None,
        skill_path: "skills/networking".into(),
        skill_folder_hash: hash.clone(),
        installed_at: None,
        updated_at: None,
        plugin_name: None,
    };
    let fingerprint = format!("{:x}", {
        use sha2::Digest;
        sha2::Sha256::digest(std::fs::read(harness.lock_path()).expect("lock bytes"))
    });
    let journal = skill_man_lib::seams::filesystem::HandoffJournal {
        version: 1,
        operation_id: "handoff-1".into(),
        staging_operation_root: harness.home.library_root.join("staging").join("handoff-1"),
        staging_fingerprint: skill_man_lib::seams::filesystem::DirectoryFingerprint {
            canonical_path: harness.home.library_root.join("staging").join("handoff-1"),
            device: 0,
            inode: 0,
        },
        items: vec![skill_man_lib::seams::filesystem::HandoffJournalItem {
            skill_id: "adopt-crash-1".into(),
            directory_name: "networking".into(),
            identity_key: "networking".into(),
            display_name: "Networking".into(),
            description: String::new(),
            canonical_entity: local_canonical.clone(),
            isolated_path: Some(isolated.clone()),
            staged_root: staged_root.clone(),
            staged_fingerprint: None,
            final_entity_path: harness.home.library_root.join("skills/networking"),
            source_tree_hash: harness.tree_hash(&isolated),
            staged_tree_hash: None,
            lock_path: harness.lock_path(),
            lock_fingerprint: fingerprint,
            lock_entry_name: "networking".into(),
            lock_entry_json: serde_json::to_string(&entry).expect("entry JSON"),
            remote_id: None,
            remote: Some(skill_man_lib::seams::filesystem::HandoffRemoteJournal {
                canonical_url: url.clone(),
                requested_ref: "HEAD".into(),
                verification_anchor_commit: head_of(&repo),
                original_commit_known: false,
                skill_path: "skills/networking".into(),
                provider_hash: Some(hash.clone()),
                remote_baseline_hash: harness.tree_hash(&isolated),
            }),
            current_baseline_hash: harness.tree_hash(&isolated),
            appearances: vec![],
            activations: vec![],
            target_directory: None,
            phase: skill_man_lib::seams::filesystem::HandoffItemPhase::SourceIsolated,
        }],
    };
    harness
        .filesystem
        .write_handoff_journal(&harness.home.library_root, &journal)
        .expect("write crash journal");

    // Startup recovery: pre-CAS → roll back in place.
    harness
        .maintenance()
        .startup_check()
        .expect("startup recovery");
    assert!(local_canonical.exists(), "external source restored");
    assert_eq!(
        std::fs::read_to_string(local_canonical.join("SKILL.md")).expect("SKILL.md"),
        "# Networking v1\n"
    );
    assert!(!isolated.exists(), "isolation copy consumed");
    assert!(!staged_root.exists(), "staged copy discarded");
    let records = harness.runtime.load_remote_installs().expect("records");
    assert!(records.is_empty(), "no catalog claim before the CAS");
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.lock_path()).expect("lock")).expect("JSON");
    assert!(
        lock["skills"]["networking"].is_object(),
        "lock untouched before the CAS"
    );
    assert!(
        !harness
            .home
            .library_root
            .join("operations")
            .join("handoff-1")
            .exists(),
        "journal archived"
    );
}

#[test]
fn crash_after_cas_rolls_forward_under_the_recovery_gate() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
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
    let tree_hash = harness.tree_hash(&harness.shared().join("networking"));

    // Emulate a crash right after the CAS: the lock entry is gone, the
    // journal is at OwnershipReleased, the staged tree exists, the source
    // sits in the isolation copy, and nothing was published yet.
    let canonical = harness
        .shared()
        .join("networking")
        .canonicalize()
        .expect("canonical");
    let isolated = harness
        .shared()
        .join(".skill-man-handoff-handoff-2-networking");
    std::fs::rename(&canonical, &isolated).expect("isolate source");
    let staged_root = harness
        .home
        .library_root
        .join("staging")
        .join("handoff-2")
        .join("networking");
    std::fs::create_dir_all(staged_root.parent().expect("staging parent")).expect("staging root");
    std::fs::create_dir_all(&staged_root).expect("staged directory");
    std::fs::write(staged_root.join("SKILL.md"), "# Networking v1\n").expect("staged copy");
    // The CAS already removed the entry: the lock is a valid empty v3.
    std::fs::write(harness.lock_path(), lock_json(&[])).expect("rewrite lock after CAS");
    let entry = skill_man_lib::seams::installer_lock_store::LockEntry {
        name: "networking".into(),
        source_type: "git".into(),
        source: url.clone(),
        source_url: url.clone(),
        requested_ref: None,
        skill_path: "skills/networking".into(),
        skill_folder_hash: hash.clone(),
        installed_at: None,
        updated_at: None,
        plugin_name: None,
    };
    let journal = skill_man_lib::seams::filesystem::HandoffJournal {
        version: 1,
        operation_id: "handoff-2".into(),
        staging_operation_root: harness.home.library_root.join("staging").join("handoff-2"),
        staging_fingerprint: skill_man_lib::seams::filesystem::DirectoryFingerprint {
            canonical_path: harness.home.library_root.join("staging").join("handoff-2"),
            device: 0,
            inode: 0,
        },
        items: vec![skill_man_lib::seams::filesystem::HandoffJournalItem {
            skill_id: "adopt-crash-2".into(),
            directory_name: "networking".into(),
            identity_key: "networking".into(),
            display_name: "Networking".into(),
            description: String::new(),
            canonical_entity: harness.shared().join("networking"),
            isolated_path: Some(isolated.clone()),
            staged_root: staged_root.clone(),
            staged_fingerprint: None,
            final_entity_path: harness.home.library_root.join("skills/networking"),
            source_tree_hash: tree_hash.clone(),
            staged_tree_hash: Some(tree_hash.clone()),
            lock_path: harness.lock_path(),
            lock_fingerprint: String::new(),
            lock_entry_name: "networking".into(),
            lock_entry_json: serde_json::to_string(&entry).expect("entry JSON"),
            remote_id: None,
            remote: Some(skill_man_lib::seams::filesystem::HandoffRemoteJournal {
                canonical_url: url.clone(),
                requested_ref: "HEAD".into(),
                verification_anchor_commit: head_of(&repo),
                original_commit_known: false,
                skill_path: "skills/networking".into(),
                provider_hash: Some(hash.clone()),
                remote_baseline_hash: tree_hash.clone(),
            }),
            current_baseline_hash: tree_hash.clone(),
            appearances: vec![],
            activations: vec![],
            target_directory: None,
            phase: skill_man_lib::seams::filesystem::HandoffItemPhase::OwnershipReleased,
        }],
    };
    harness
        .filesystem
        .write_handoff_journal(&harness.home.library_root, &journal)
        .expect("write crash journal");

    // Startup recovery under the recovery gate: roll forward only.
    harness
        .maintenance()
        .startup_check()
        .expect("startup recovery");
    let home_entity = harness.home.library_root.join("skills/networking");
    assert_eq!(
        std::fs::read_to_string(home_entity.join("SKILL.md")).expect("SKILL.md"),
        "# Networking v1\n",
        "the Home entity is published"
    );
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].directory_name, "networking");
    assert_eq!(records[0].verification_anchor_commit, head_of(&repo));
    let parent = harness
        .runtime
        .find_remote_parent_by_url(&url)
        .expect("parent lookup")
        .expect("parent exists");
    let manifest = harness
        .filesystem
        .read_remote_parent_manifest(
            &harness.home.library_root.join("remotes"),
            &parent.remote_id,
        )
        .expect("manifest read")
        .expect("manifest exists");
    assert_eq!(manifest.canonical_url, url);
    assert!(
        !isolated.exists(),
        "isolation copy discarded after the restart closed the window"
    );
    assert!(!staged_root.exists(), "staged copy consumed");
    assert!(
        !harness
            .home
            .library_root
            .join("operations")
            .join("handoff-2")
            .exists(),
        "journal archived"
    );
}

#[test]
fn crash_between_cas_and_journal_write_rolls_forward_not_back() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
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
    let tree_hash = harness.tree_hash(&harness.shared().join("networking"));

    // Emulate the crash window between the exact-entry CAS (the commit
    // point) and the durable journal phase write: the lock entry is gone,
    // but the journal still says SourceIsolated.
    let canonical = harness
        .shared()
        .join("networking")
        .canonicalize()
        .expect("canonical");
    let isolated = harness
        .shared()
        .join(".skill-man-handoff-handoff-3-networking");
    std::fs::rename(&canonical, &isolated).expect("isolate source");
    let staged_root = harness
        .home
        .library_root
        .join("staging")
        .join("handoff-3")
        .join("networking");
    std::fs::create_dir_all(staged_root.parent().expect("staging parent")).expect("staging root");
    std::fs::create_dir_all(&staged_root).expect("staged directory");
    std::fs::write(staged_root.join("SKILL.md"), "# Networking v1\n").expect("staged copy");
    // The CAS already removed the entry.
    std::fs::write(harness.lock_path(), lock_json(&[])).expect("rewrite lock after CAS");
    let entry = skill_man_lib::seams::installer_lock_store::LockEntry {
        name: "networking".into(),
        source_type: "git".into(),
        source: url.clone(),
        source_url: url.clone(),
        requested_ref: None,
        skill_path: "skills/networking".into(),
        skill_folder_hash: hash.clone(),
        installed_at: None,
        updated_at: None,
        plugin_name: None,
    };
    let journal = skill_man_lib::seams::filesystem::HandoffJournal {
        version: 1,
        operation_id: "handoff-3".into(),
        staging_operation_root: harness.home.library_root.join("staging").join("handoff-3"),
        staging_fingerprint: skill_man_lib::seams::filesystem::DirectoryFingerprint {
            canonical_path: harness.home.library_root.join("staging").join("handoff-3"),
            device: 0,
            inode: 0,
        },
        items: vec![skill_man_lib::seams::filesystem::HandoffJournalItem {
            skill_id: "adopt-crash-3".into(),
            directory_name: "networking".into(),
            identity_key: "networking".into(),
            display_name: "Networking".into(),
            description: String::new(),
            canonical_entity: harness.shared().join("networking"),
            isolated_path: Some(isolated.clone()),
            staged_root: staged_root.clone(),
            staged_fingerprint: None,
            final_entity_path: harness.home.library_root.join("skills/networking"),
            source_tree_hash: tree_hash.clone(),
            staged_tree_hash: Some(tree_hash.clone()),
            lock_path: harness.lock_path(),
            lock_fingerprint: String::new(),
            lock_entry_name: "networking".into(),
            lock_entry_json: serde_json::to_string(&entry).expect("entry JSON"),
            remote_id: None,
            remote: Some(skill_man_lib::seams::filesystem::HandoffRemoteJournal {
                canonical_url: url.clone(),
                requested_ref: "HEAD".into(),
                verification_anchor_commit: head_of(&repo),
                original_commit_known: false,
                skill_path: "skills/networking".into(),
                provider_hash: Some(hash.clone()),
                remote_baseline_hash: tree_hash.clone(),
            }),
            current_baseline_hash: tree_hash.clone(),
            appearances: vec![],
            activations: vec![],
            target_directory: None,
            phase: skill_man_lib::seams::filesystem::HandoffItemPhase::SourceIsolated,
        }],
    };
    harness
        .filesystem
        .write_handoff_journal(&harness.home.library_root, &journal)
        .expect("write crash journal");

    // Recovery resolves the direction from the lock itself: the entry is
    // gone, so the CAS passed and the item rolls FORWARD (spec §8.4: CAS
    // 后只 roll-forward; 无长期双 owner 或无人 owner).
    harness
        .maintenance()
        .startup_check()
        .expect("startup recovery");
    let home_entity = harness.home.library_root.join("skills/networking");
    assert_eq!(
        std::fs::read_to_string(home_entity.join("SKILL.md")).expect("SKILL.md"),
        "# Networking v1\n",
        "the Home entity is published, never restored to the installer"
    );
    assert!(!isolated.exists(), "isolation copy consumed");
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records.len(), 1);
}

// =====================================================================
// Update through the parent model + last-child Remove
// =====================================================================

#[test]
fn update_flow_tracks_the_binding_and_last_child_remove_deletes_the_parent() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
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
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "networking", None, None)],
        })
        .expect("plan");
    adopt.apply(&plan.plan_token).expect("apply");

    // Upstream advances; the check resolves the moving ref against the
    // binding's anchor.
    let anchor = head_of(&repo);
    let second = commit_file(&repo, "skills/networking/SKILL.md", "# Networking v2\n");
    let update = harness.update();
    let report = update.check_updates(true).expect("check updates");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let item = &report.groups[0].items[0];
    assert!(item.has_update);
    assert_eq!(item.current_commit, anchor);
    // The real skill id comes from the catalog.
    let records = harness.runtime.load_remote_installs().expect("records");
    let plan = update
        .plan_updates(&[UpdateSelection {
            skill_id: records[0].skill_id.clone(),
            new_skill_path: None,
        }])
        .expect("plan update");
    assert!(plan.items[0].error.is_none(), "{:?}", plan.items[0].error);
    let result = update
        .apply_updates(
            &[skill_man_lib::core::update::UpdateApplyRequest {
                plan_token: plan.items[0].plan_token.clone(),
                skill_id: records[0].skill_id.clone(),
                directory_name: "networking".into(),
            }],
            true,
        )
        .expect("apply update");
    assert!(result.items[0].updated, "{:?}", result.items[0].error);
    let records = harness
        .runtime
        .load_remote_installs()
        .expect("records after update");
    assert_eq!(records[0].verification_anchor_commit, second);
    assert_eq!(
        std::fs::read_to_string(harness.home.library_root.join("skills/networking/SKILL.md"))
            .expect("SKILL.md"),
        "# Networking v2\n"
    );

    // Last-child Remove: the binding cascades, the empty parent row and
    // manifest are deleted; the external lock is NOT restored.
    let maintenance = harness.maintenance();
    let remove = maintenance
        .plan_remove(&records[0].skill_id)
        .expect("plan remove");
    maintenance
        .apply_remove(&remove.plan_token)
        .expect("apply remove");
    let records = harness.runtime.load_remote_installs().expect("records");
    assert!(records.is_empty());
    assert!(
        harness
            .runtime
            .find_remote_parent_by_url(&url)
            .expect("parent lookup")
            .is_none(),
        "empty parent row deleted"
    );
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.lock_path()).expect("lock")).expect("JSON");
    assert!(
        lock["skills"].as_object().expect("skills").is_empty(),
        "the old external owner is never restored"
    );
}

// =====================================================================
// External installer reappearance → Ownership Conflict
// =====================================================================

#[test]
fn external_installer_reappearance_is_ownership_conflict_never_update() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
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
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "networking", None, None)],
        })
        .expect("plan");
    adopt.apply(&plan.plan_token).expect("apply");

    // The external installer reappears: a fresh canonical directory plus a
    // lock declaration for the same name.
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
    .expect("rewrite lock");

    let report = adopt.scan().expect("rescan");
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| candidate.directory_name == "networking")
        .expect("candidate");
    assert_eq!(candidate.verdict, AdoptVerdict::Conflict);
    assert!(matches!(
        candidate.reason,
        Some(AdoptVerdictReason::OwnershipConflict { .. })
    ));
    assert!(!candidate.selectable, "never auto-taken-over");
    // The Managed Skill and its Home bytes are untouched.
    assert_eq!(
        std::fs::read_to_string(harness.home.library_root.join("skills/networking/SKILL.md"))
            .expect("SKILL.md"),
        "# Networking v1\n"
    );
    // The update check never reports it as an Update.
    let report = harness.update().check_updates(true).expect("check");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let records = harness.runtime.load_remote_installs().expect("records");
    let checked = report
        .groups
        .iter()
        .flat_map(|group| group.items.iter())
        .map(|item| item.skill_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        checked,
        vec![records[0].skill_id.clone()],
        "the check tracks only the Managed Skill; the external copy is never an Update"
    );
}

// =====================================================================
// Conditional Undo guards
// =====================================================================

#[test]
fn undo_refuses_when_the_external_canonical_location_is_occupied() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo = fixture_repo(
        root.path(),
        &[("skills/networking/SKILL.md", "# Networking v1\n")],
    );
    let url = format!("file://{}", repo.display());
    let hash = cli_hash_at(&repo, "HEAD", "skills/networking");
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
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "networking", None, None)],
        })
        .expect("plan");
    let result = adopt.apply(&plan.plan_token).expect("apply");

    // New external work occupies the canonical location: Undo must refuse
    // and keep the committed state.
    write_skill(&harness.shared(), "networking", "# New external work\n");
    let undo = adopt.undo(&result.operation_id).expect("undo attempted");
    assert_eq!(undo.items.len(), 1);
    assert!(!undo.items[0].undone);
    assert!(
        undo.items[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("occupied")),
        "{:?}",
        undo.items[0].error
    );
    // The committed Managed Skill stays.
    assert!(harness.home.library_root.join("skills/networking").exists());
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records.len(), 1);
    // The lock entry stays released.
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.lock_path()).expect("lock")).expect("JSON");
    assert!(lock["skills"].as_object().expect("skills").is_empty());
}

// =====================================================================
// URL alias confirmation and two-parent convergence fail-closed
// =====================================================================

#[test]
fn confirmed_alias_reuses_the_parent_and_convergence_fails_closed() {
    let harness = Harness::new();
    let root = tempfile::tempdir().expect("temp repo root");
    let repo_a = fixture_repo(
        root.path().join("a").as_path(),
        &[
            ("skills/networking/SKILL.md", "# Networking v1\n"),
            ("skills/gamma/SKILL.md", "# Gamma v1\n"),
        ],
    );
    let url_a = format!("file://{}", repo_a.display());
    let hash_a = cli_hash_at(&repo_a, "HEAD", "skills/networking");
    let hash_gamma = cli_hash_at(&repo_a, "HEAD", "skills/gamma");
    write_skill(&harness.shared(), "networking", "# Networking v1\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "networking",
            "git",
            url_a.clone(),
            None,
            "skills/networking".into(),
            hash_a.clone(),
        )]),
    )
    .expect("write lock");
    let adopt = harness.adopt();
    let report = adopt.scan().expect("scan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "networking", None, None)],
        })
        .expect("plan");
    adopt.apply(&plan.plan_token).expect("apply handoff A");
    let parent_a = harness
        .runtime
        .find_remote_parent_by_url(&url_a)
        .expect("parent lookup")
        .expect("parent A exists");

    // The repository was renamed: the user confirms the redirect as an
    // alias of the same remote_id (ADR-0013 §4.2).
    let alias = "https://github.com/acme/skills".to_string();
    harness
        .runtime
        .insert_remote_alias(&parent_a.remote_id, &alias)
        .expect("confirm alias");
    let via_alias = harness
        .runtime
        .find_remote_parent_by_url(&alias)
        .expect("alias lookup")
        .expect("alias resolves to the parent");
    assert_eq!(via_alias.remote_id, parent_a.remote_id);
    assert_eq!(via_alias.aliases, vec![alias.clone()]);

    // A new Skill from the same repository reuses the parent via its
    // canonical URL; the manifest is refreshed with the confirmed alias.
    write_skill(&harness.shared(), "gamma", "# Gamma v1\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "gamma",
            "git",
            url_a.clone(),
            None,
            "skills/gamma".into(),
            hash_gamma.clone(),
        )]),
    )
    .expect("write lock for gamma");
    let report = adopt.scan().expect("rescan");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "gamma", None, None)],
        })
        .expect("plan");
    let result = adopt.apply(&plan.plan_token).expect("apply handoff gamma");
    assert!(result.items[0].adopted, "{:?}", result.items[0].error);
    let records = harness.runtime.load_remote_installs().expect("records");
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.remote_id == parent_a.remote_id)
    );

    // A second repository converges onto the alias? No: its canonical URL
    // already belongs to another parent, so the alias confirmation fails
    // closed (ADR-0013 §4.2: two existing parents never auto-merge; the
    // user must explicitly choose a survivor and re-verify per Skill).
    let repo_b = fixture_repo(
        root.path().join("b").as_path(),
        &[("skills/audio/SKILL.md", "# Audio v1\n")],
    );
    let url_b = format!("file://{}", repo_b.display());
    let hash_b = cli_hash_at(&repo_b, "HEAD", "skills/audio");
    write_skill(&harness.shared(), "audio", "# Audio v1\n");
    std::fs::write(
        harness.lock_path(),
        lock_json(&[(
            "audio",
            "git",
            url_b.clone(),
            None,
            "skills/audio".into(),
            hash_b.clone(),
        )]),
    )
    .expect("write lock for audio");
    let report = adopt.scan().expect("rescan for audio");
    let plan = adopt
        .plan(&AdoptPlanRequest {
            evidence_generation: report.generation,
            selections: vec![harness.select(&report.candidates, "audio", None, None)],
        })
        .expect("plan audio");
    adopt.apply(&plan.plan_token).expect("apply handoff audio");
    let parent_b = harness
        .runtime
        .find_remote_parent_by_url(&url_b)
        .expect("parent lookup")
        .expect("parent B exists");
    assert_ne!(parent_a.remote_id, parent_b.remote_id);
    let converge = harness
        .runtime
        .insert_remote_alias(&parent_a.remote_id, &url_b)
        .expect_err("the alias collides with an existing parent");
    assert!(
        converge.to_string().contains("never auto-merge"),
        "{converge}"
    );

    // The handoff that reused nothing new wrote the manifest with the
    // confirmed alias; the Update integrity gate accepts it.
    let manifest = harness
        .filesystem
        .read_remote_parent_manifest(
            &harness.home.library_root.join("remotes"),
            &parent_a.remote_id,
        )
        .expect("manifest read")
        .expect("manifest exists");
    assert_eq!(manifest.aliases, vec![alias.clone()]);
    let report = harness.update().check_updates(true).expect("check");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        report.parent_conflicts.is_empty(),
        "{:?}",
        report.parent_conflicts
    );
}
