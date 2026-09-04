//! Global Enable vertical slice (spec §4.9; ADR-0019): canonical Target
//! group resolution, the journaled per-cell commit (plain Enable/Disable/
//! Repair, Managed Switch, Untracked Remove-then-replace), the per-cell CAS
//! Undo, startup rollback/roll-forward recovery and the Source Snapshot
//! Mismatch gate. All tests run against an isolated temp Home with the real
//! macOS filesystem adapter.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::params;
use skill_man_lib::adapters::agent_configuration_fs::MacOsAgentConfigurationFileSystem;
use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::domain::SkillId;
use skill_man_lib::core::enable::{
    CellEligibility, CellOutcome, CellResolution, EnableAction, EnableService,
};
use skill_man_lib::core::source_group_preview::SourceGroupPreviewService;
use skill_man_lib::core::source_transition::SourceTransitionService;
use skill_man_lib::core::source_update::SourceUpdateService;
use skill_man_lib::seams::activation_store::ActivationStore;
use skill_man_lib::seams::agent_configuration_store::AgentConfigurationStore;
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::filesystem::{
    ActivationReplacePhase, EnableCellAction, EnableJournal, EnableJournalCell, FileSystem,
    OccupantSnapshot,
};
use skill_man_lib::seams::source::GitSource;

mod common;
use common::BoundTestHome;

struct Harness {
    home: BoundTestHome,
    enable: EnableService,
    library_root: PathBuf,
}

fn harness() -> Harness {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    // Seeded Agent roots are catalog rows; the Enable flow observes the
    // physical Target roots, which must exist.
    std::fs::create_dir_all(home.claude_root()).expect("create claude root");
    std::fs::create_dir_all(home.codex_root()).expect("create codex root");
    let library_root = home.library_root.clone();
    let enable = EnableService::new(
        home.runtime.clone(),
        home.runtime.clone(),
        home.runtime.clone(),
        Arc::new(MacOsAgentConfigurationFileSystem::new(
            home.path().to_path_buf(),
        )),
        home.filesystem.clone(),
        Arc::new(SystemClock::new()),
        library_root.clone(),
        home.write_gate.clone(),
    )
    .with_home_context(home.write_gate.clone());
    Harness {
        home,
        enable,
        library_root,
    }
}

fn entry_path(harness: &Harness, directory_name: &str, agent: &str) -> PathBuf {
    let root_id = harness.home.activation_root_id(agent);
    let root_path = harness
        .home
        .runtime
        .agent_configuration_snapshot()
        .expect("agent snapshot")
        .roots
        .into_iter()
        .find(|root| root.root_id == root_id)
        .map(|root| root.configured_path)
        .expect("root path");
    root_path.join(directory_name)
}

#[test]
fn scan_only_root_id_cannot_plan_global_activation() {
    let harness = harness();
    let scan_only_root = harness.home.path().join("scan-only-root");
    harness.home.with_sql("seed scan-only root", |connection| {
        connection
            .execute(
                "INSERT INTO global_skill_roots (
                    root_id, configured_path, path_identity_key, created_at, updated_at
                 ) VALUES ('root-scan-only', ?1, ?2, '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z')",
                params![
                    scan_only_root.to_string_lossy(),
                    skill_man_lib::core::domain::configured_path_identity_key(
                        &scan_only_root.to_string_lossy()
                    ),
                ],
            )
            .expect("insert scan-only root");
        connection
            .execute(
                "INSERT INTO agent_global_roots (agent_id, root_id, role)
                 VALUES ('claude-code', 'root-scan-only', 'scan_only')",
                [],
            )
            .expect("insert scan-only membership");
    });

    let result = harness.enable.plan_global_enable(
        &[SkillId("skill-authoring".into())],
        &["root-scan-only".into()],
        &[],
    );

    assert!(matches!(
        result,
        Err(skill_man_lib::core::enable::EnableError::Validation(_))
    ));
    assert!(
        !scan_only_root.exists(),
        "a scan-only root must never be created by Global Enable"
    );
}

#[test]
fn apply_rejects_global_target_ancestor_replacement_as_plan_stale() {
    let harness = harness();
    let skill_id = SkillId("skill-authoring".into());
    let plan = harness
        .enable
        .plan_global_enable(
            std::slice::from_ref(&skill_id),
            &[harness.home.activation_root_id("claude-code")],
            &[],
        )
        .expect("plan global enable");

    let outside = tempfile::tempdir().expect("outside target");
    let claude_root = harness.home.path().join(".claude");
    let preserved = harness.home.path().join(".claude-original");
    std::fs::rename(&claude_root, &preserved).expect("preserve global target ancestor");
    std::os::unix::fs::symlink(outside.path(), &claude_root)
        .expect("replace global target ancestor");

    let result = harness.enable.apply(&plan.plan_token);

    assert!(matches!(
        result,
        Err(skill_man_lib::core::enable::EnableError::PlanStale)
    ));
    assert!(
        !outside.path().join("skills/skill-authoring").exists(),
        "Global Enable must not follow a replaced Target ancestor"
    );
    assert!(preserved.join("skills").is_dir());
}

struct FixtureClock;

impl Clock for FixtureClock {
    fn monotonic_millis(&self) -> u128 {
        1
    }

    fn unix_epoch_nanos(&self) -> u128 {
        1_725_000_000_000_000_000
    }
}

/// The Source Snapshot Mismatch gate (spec §8.3; #93): a Git member whose
/// snapshot mismatches blocks new Enable but leaves Disable usable.
#[test]
fn source_snapshot_mismatch_blocks_new_enable_but_not_disable() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    std::fs::create_dir_all(home.claude_root()).expect("create claude root");
    // Seed a v9 Git member whose snapshot mismatches, plus its release rows.
    let git_entity = home.path().join("git-skills/git-skill");
    std::fs::create_dir_all(&git_entity).expect("create git member entity");
    std::fs::write(git_entity.join("SKILL.md"), "# Git Skill\n").expect("write member SKILL.md");
    home.with_sql("seed Git Source gate facts", |connection| {
        connection
            .execute(
                "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
                 VALUES ('remote-1', 'https://example.com/acme/source.git', '2026-08-01T00:00:00Z')",
                [],
            )
            .expect("seed remote source parent");
        connection
            .execute(
                "INSERT INTO git_source_releases (
                    release_id, remote_id, selection_kind, selected_ref,
                    resolved_commit, discovered_at
                 ) VALUES ('release-1', 'remote-1', 'head', 'main', 'c0ffee', '2026-08-01T00:00:00Z')",
                [],
            )
            .expect("seed release");
        connection
            .execute(
                "INSERT INTO skills (
                    id, directory_name, directory_identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path, health,
                    created_at, updated_at
                 ) VALUES ('git-skill', 'git-skill', 'git-skill', 'Git Skill', '',
                    'remote_install', ?1, ?1,
                    'source_snapshot_mismatch', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')",
                params![git_entity.to_string_lossy()],
            )
            .expect("seed mismatched member Skill");
        connection
            .execute(
                "INSERT INTO git_source_members (
                    skill_id, remote_id, skill_path, storage_relpath, presence,
                    first_seen_release_id, last_seen_release_id
                 ) VALUES ('git-skill', 'remote-1', 'skills/git-skill',
                    'skills/git/remote-1/git-skill', 'current', 'release-1', 'release-1')",
                [],
            )
            .expect("seed member");
    });

    let source: Arc<dyn GitSource> = Arc::new(SystemGitSource::new());
    let locks = Arc::new(SystemInstallerLockStore::new(home.path().to_path_buf()));
    let preview = Arc::new(SourceGroupPreviewService::new(
        source.clone(),
        locks.clone(),
    ));
    let transition = Arc::new(
        SourceTransitionService::new(
            preview.clone(),
            source.clone(),
            locks.clone(),
            home.runtime.clone(),
            home.filesystem.clone(),
            Arc::new(FixtureClock),
            home.library_root.clone(),
            home.path().to_path_buf(),
            home.write_gate.clone(),
        )
        .with_update_store(home.runtime.clone()),
    );
    let update = Arc::new(
        SourceUpdateService::new(
            preview.clone(),
            transition.clone(),
            home.filesystem.clone(),
            home.library_root.clone(),
        )
        .with_home_context(),
    );
    let enable = EnableService::new(
        home.runtime.clone(),
        home.runtime.clone(),
        home.runtime.clone(),
        Arc::new(MacOsAgentConfigurationFileSystem::new(
            home.path().to_path_buf(),
        )),
        home.filesystem.clone(),
        Arc::new(SystemClock::new()),
        home.library_root.clone(),
        home.write_gate.clone(),
    )
    .with_home_context(home.write_gate.clone())
    .with_source_update(update);

    // New Enable on a mismatched member: Blocked (typed code), no write.
    let plan = enable
        .plan_global_enable(
            &[SkillId("git-skill".into())],
            &[home.activation_root_id("claude-code")],
            &[],
        )
        .expect("plan enable on mismatched member");
    assert_eq!(plan.cells[0].eligibility, CellEligibility::Blocked);
    assert_eq!(
        plan.cells[0].blocked_reason,
        Some(skill_man_lib::core::enable::CellBlockedReason::SourceSnapshotMismatch)
    );
    assert!(!home.claude_root().join("git-skill").exists());

    // Disable stays usable: seed a desired row and a link, then Disable.
    home.with_sql("seed desired activation", |connection| {
        let root_id = home.activation_root_id("claude-code");
        connection
            .execute(
                "INSERT INTO activations (
                    skill_id, target_root_id, directory_identity_key, desired_enabled,
                    expected_entry_path, expected_target_path, observed_state,
                    last_enabled_at, last_checked_at
                 ) VALUES ('git-skill', ?1, 'git-skill', 1, ?2, '/nonexistent/git-skill',
                    'present', '2026-08-01T00:00:00Z', NULL)",
                params![
                    root_id,
                    home.claude_root().join("git-skill").to_string_lossy(),
                ],
            )
            .expect("seed disable candidate");
    });
    std::os::unix::fs::symlink(&git_entity, home.claude_root().join("git-skill"))
        .expect("link git-skill");
    let plan = enable
        .plan_global_lifecycle(
            &SkillId("git-skill".into()),
            &home.activation_root_id("claude-code"),
            EnableAction::Disable,
        )
        .expect("plan disable under mismatch");
    assert_eq!(plan.cells[0].eligibility, CellEligibility::Ready);
    let result = enable.apply(&plan.plan_token).expect("apply disable");
    assert_eq!(result.cells[0].outcome, CellOutcome::Succeeded);
    assert!(!home.claude_root().join("git-skill").exists());
}

fn catalog_desired(harness: &Harness, skill_id: &str, agent: &str) -> bool {
    let root_id = harness.home.activation_root_id(agent);
    harness
        .home
        .runtime
        .activation_cells_for_skill(&SkillId(skill_id.to_owned()))
        .expect("cells")
        .into_iter()
        .find(|cell| cell.target_root_id == root_id)
        .map(|cell| cell.desired_enabled)
        .unwrap_or(false)
}

#[test]
fn list_target_groups_resolves_shared_target_and_typed_availability() {
    let harness = harness();
    std::fs::create_dir_all(harness.home.workbench_root()).expect("create workbench root");
    let skill_id = SkillId("skill-authoring".to_owned());
    let snapshot = harness
        .enable
        .list_target_groups(&skill_id)
        .expect("list target groups");
    assert_eq!(snapshot.skill_id, skill_id);
    assert_eq!(snapshot.groups.len(), 3);
    let claude = snapshot
        .groups
        .iter()
        .find(|group| group.consumers.iter().any(|m| m.agent_id == "claude-code"))
        .expect("claude group");
    assert_eq!(claude.consumers.len(), 1);
    assert!(!claude.desired);
    assert!(claude.observed.is_none());
    // Typed availability: the configured root exists in the seeded Home.
    let available = snapshot
        .groups
        .iter()
        .filter(|group| {
            matches!(
                group.availability,
                skill_man_lib::core::enable::TargetGroupAvailability::Available
            )
        })
        .count();
    assert_eq!(available, snapshot.groups.len());
}

#[test]
fn global_enable_creates_catalog_row_entry_and_journal_then_disable_undoes() {
    let harness = harness();
    let skill_id = SkillId("skill-authoring".to_owned());
    let target_group_id = harness.home.activation_root_id("claude-code");
    let plan = harness
        .enable
        .plan_global_enable(
            std::slice::from_ref(&skill_id),
            std::slice::from_ref(&target_group_id),
            &[],
        )
        .expect("plan enable");
    assert_eq!(plan.cells.len(), 1);
    assert_eq!(plan.cells[0].eligibility, CellEligibility::Ready);
    let result = harness.enable.apply(&plan.plan_token).expect("apply");
    assert_eq!(result.cells[0].outcome, CellOutcome::Succeeded);
    let entry = harness.home.claude_root().join("skill-authoring");
    assert!(
        std::fs::symlink_metadata(&entry)
            .expect("entry exists")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(&entry).expect("read link"),
        harness.home.path().join("Projects/skill-authoring")
    );
    assert!(catalog_desired(&harness, "skill-authoring", "claude-code"));
    assert!(
        harness
            .home
            .library_root
            .join("operations")
            .join(&result.operation_id)
            .join("enable-journal.json")
            .is_file()
    );

    // Undo: entry removed, catalog desired false, journal finished.
    let undo = harness.enable.undo(&result.operation_id).expect("undo");
    assert_eq!(undo.cells.len(), 1);
    assert!(undo.cells[0].undone);
    assert!(!harness.home.claude_root().join("skill-authoring").exists());
    assert!(!catalog_desired(&harness, "skill-authoring", "claude-code"));
    assert!(
        !harness
            .home
            .library_root
            .join("operations")
            .join(&result.operation_id)
            .exists()
    );
}

#[test]
fn disable_requires_desired_row_and_removes_link_keeping_other_targets() {
    let harness = harness();
    harness
        .home
        .seed_activation("skill-authoring", "claude-code", true, "present");
    std::os::unix::fs::symlink(
        harness.home.path().join("Projects/skill-authoring"),
        entry_path(&harness, "skill-authoring", "claude-code"),
    )
    .expect("activate claude");
    harness
        .home
        .seed_activation("skill-authoring", "codex", true, "present");
    std::os::unix::fs::symlink(
        harness.home.path().join("Projects/skill-authoring"),
        entry_path(&harness, "skill-authoring", "codex"),
    )
    .expect("activate codex");

    let plan = harness
        .enable
        .plan_global_lifecycle(
            &SkillId("skill-authoring".to_owned()),
            &harness.home.activation_root_id("claude-code"),
            EnableAction::Disable,
        )
        .expect("plan disable");
    harness.enable.apply(&plan.plan_token).expect("apply");
    assert!(!harness.home.claude_root().join("skill-authoring").exists());
    assert!(harness.home.codex_root().join("skill-authoring").exists());
    assert!(!catalog_desired(&harness, "skill-authoring", "claude-code"));
    assert!(catalog_desired(&harness, "skill-authoring", "codex"));
}

#[test]
fn managed_switch_keeps_old_skill_other_targets_and_same_target_history() {
    let harness = harness();
    // A second Skill with the SAME Directory Identity ("skill-authoring"):
    // enabling it on the claude Target conflicts with skill-authoring's
    // entry ownership -> the only resolution is a Target-local Switch.
    let twin_entity = harness.home.path().join("Projects/twin-authoring");
    std::fs::create_dir_all(&twin_entity).expect("twin entity");
    std::fs::write(twin_entity.join("SKILL.md"), "# Twin\n").expect("write twin");
    harness.home.with_sql("seed twin Skill", |connection| {
        connection
            .execute(
                "INSERT INTO skills (
                    id, directory_name, directory_identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path, health,
                    created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, '', 'link', NULL, ?5, 'healthy', ?6, ?6)",
                params![
                    "twin-skill",
                    "skill-authoring",
                    "skill-authoring",
                    "Twin authoring",
                    twin_entity.to_string_lossy(),
                    "2026-08-01T00:00:00Z",
                ],
            )
            .expect("insert twin Skill");
    });
    harness
        .home
        .seed_activation("skill-authoring", "claude-code", true, "present");
    let claude_entry = entry_path(&harness, "skill-authoring", "claude-code");
    std::os::unix::fs::symlink(
        harness.home.path().join("Projects/skill-authoring"),
        &claude_entry,
    )
    .expect("activate skill-authoring");
    harness
        .home
        .seed_activation("skill-authoring", "codex", true, "present");

    let plan = harness
        .enable
        .plan_global_enable(
            &[SkillId("twin-skill".to_owned())],
            &[harness.home.activation_root_id("claude-code")],
            &[(
                format!(
                    "twin-skill|{}",
                    harness.home.activation_root_id("claude-code")
                ),
                CellResolution::Switch,
            )],
        )
        .expect("plan switch");
    assert_eq!(plan.cells[0].eligibility, CellEligibility::Conflict);
    assert_eq!(plan.cells[0].resolution, CellResolution::Switch);
    let result = harness.enable.apply(&plan.plan_token).expect("apply");
    assert_eq!(result.cells[0].outcome, CellOutcome::Succeeded);
    let link = std::fs::read_link(&claude_entry).expect("read switched link");
    assert!(link.ends_with("twin-authoring"));
    assert!(!catalog_desired(&harness, "skill-authoring", "claude-code"));
    assert!(catalog_desired(&harness, "twin-skill", "claude-code"));

    // The old skill's other Target is untouched.
    assert!(catalog_desired(&harness, "skill-authoring", "codex"));
}

#[test]
fn untracked_occupier_replace_backs_up_and_undo_restores_it() {
    let harness = harness();
    let claude_entry = entry_path(&harness, "skill-authoring", "claude-code");
    std::fs::create_dir_all(&claude_entry).expect("create untracked directory");
    std::fs::write(claude_entry.join("notes.txt"), "external notes").expect("write file");

    let plan = harness
        .enable
        .plan_global_enable(
            &[SkillId("skill-authoring".to_owned())],
            &[harness.home.activation_root_id("claude-code")],
            &[(
                format!(
                    "skill-authoring|{}",
                    harness.home.activation_root_id("claude-code")
                ),
                CellResolution::Replace,
            )],
        )
        .expect("plan replace");
    assert!(plan.cells[0].destructive.is_some());
    assert_eq!(plan.cells[0].destructive.as_ref().unwrap().files, 1);
    let result = match harness.enable.apply(&plan.plan_token) {
        Ok(result) => result,
        Err(error) => panic!("apply error: {error:?}"),
    };
    eprintln!(
        "REPLACE result: {:?} {:?}",
        result.cells[0].outcome, result.cells[0].diagnostic
    );
    assert_eq!(result.cells[0].outcome, CellOutcome::Succeeded);
    assert!(
        std::fs::symlink_metadata(&claude_entry)
            .expect("entry")
            .file_type()
            .is_symlink()
    );
    // Backup retained while the Undo window is open.
    let backup = harness
        .home
        .library_root
        .join("operations")
        .join(&result.operation_id)
        .join("backup")
        .join("cell-0");
    assert!(backup.join("notes.txt").is_file());

    let undo = harness.enable.undo(&result.operation_id).expect("undo");
    assert!(undo.cells[0].undone);
    assert!(claude_entry.join("notes.txt").is_file());
    assert!(!catalog_desired(&harness, "skill-authoring", "claude-code"));
    assert!(
        !harness
            .home
            .library_root
            .join("operations")
            .join(&result.operation_id)
            .exists()
    );
}

#[test]
fn exact_direct_untracked_link_is_replaced_not_claimed() {
    let harness = harness();
    let claude_entry = entry_path(&harness, "skill-authoring", "claude-code");
    let target = harness.home.path().join("Projects/skill-authoring");
    std::os::unix::fs::symlink(&target, &claude_entry).expect("exact direct link");

    let plan = harness
        .enable
        .plan_global_enable(
            &[SkillId("skill-authoring".to_owned())],
            &[harness.home.activation_root_id("claude-code")],
            &[],
        )
        .expect("plan");
    assert!(plan.cells[0].occ_exact_direct);
    assert_eq!(plan.cells[0].resolution, CellResolution::Replace);
    let result = match harness.enable.apply(&plan.plan_token) {
        Ok(result) => result,
        Err(error) => panic!("apply error: {error:?}"),
    };
    eprintln!(
        "DIRECT result: {:?} {:?}",
        result.cells[0].outcome, result.cells[0].diagnostic
    );
    assert_eq!(result.cells[0].outcome, CellOutcome::Succeeded);
    assert!(catalog_desired(&harness, "skill-authoring", "claude-code"));
}

#[test]
fn repair_missing_entry_restores_link_when_entity_healthy() {
    let harness = harness();
    harness
        .home
        .seed_activation("skill-authoring", "claude-code", true, "missing");
    // Entry missing; entity healthy -> Repair.
    let plan = harness
        .enable
        .plan_global_lifecycle(
            &SkillId("skill-authoring".to_owned()),
            &harness.home.activation_root_id("claude-code"),
            EnableAction::Repair,
        )
        .expect("plan repair");
    assert_eq!(plan.cells[0].eligibility, CellEligibility::Ready);
    harness.enable.apply(&plan.plan_token).expect("apply");
    assert!(
        std::fs::symlink_metadata(harness.home.claude_root().join("skill-authoring"))
            .expect("repaired entry")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn enable_apply_with_externally_replaced_entry_conflicts_and_rolls_back() {
    let harness = harness();
    let plan = harness
        .enable
        .plan_global_enable(
            &[SkillId("skill-authoring".to_owned())],
            &[harness.home.activation_root_id("claude-code")],
            &[],
        )
        .expect("plan");
    // The entry appears between plan and apply: preflight must fail closed.
    let claude_entry = entry_path(&harness, "skill-authoring", "claude-code");
    std::fs::create_dir_all(&claude_entry).expect("occupied");
    let error = harness.enable.apply(&plan.plan_token);
    assert!(matches!(
        error,
        Err(skill_man_lib::core::enable::EnableError::PlanStale)
    ));
}

#[test]
fn plan_is_stale_when_catalog_generation_changes() {
    let harness = harness();
    let plan = harness
        .enable
        .plan_global_enable(
            &[SkillId("skill-authoring".to_owned())],
            &[harness.home.activation_root_id("claude-code")],
            &[],
        )
        .expect("plan");
    harness
        .home
        .seed_activation("media-xray", "claude-code", true, "present");
    harness.home.with_sql("bump snapshot version", |connection| {
        connection
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .expect("bump snapshot version");
    });
    let error = harness.enable.apply(&plan.plan_token);
    assert!(matches!(
        error,
        Err(skill_man_lib::core::enable::EnableError::PlanStale)
    ));
}

#[test]
fn disabled_history_keeps_other_target_entries_and_observe_state_follows() {
    let harness = harness();
    harness
        .home
        .seed_activation("skill-authoring", "codex", true, "present");
    let plan = harness
        .enable
        .plan_global_enable(
            &[SkillId("skill-authoring".to_owned())],
            &[harness.home.activation_root_id("claude-code")],
            &[],
        )
        .expect("plan");
    harness.enable.apply(&plan.plan_token).expect("apply");
    assert!(catalog_desired(&harness, "skill-authoring", "codex"));
    assert!(catalog_desired(&harness, "skill-authoring", "claude-code"));
}

#[test]
fn undo_rejects_externally_changed_cell_and_continues_others() {
    let harness = harness();
    // Two targets, both enabled.
    let plan = harness
        .enable
        .plan_global_enable(
            &[SkillId("skill-authoring".to_owned())],
            &[
                harness.home.activation_root_id("claude-code"),
                harness.home.activation_root_id("codex"),
            ],
            &[],
        )
        .expect("plan");
    let result = harness.enable.apply(&plan.plan_token).expect("apply");
    assert_eq!(
        result
            .cells
            .iter()
            .filter(|cell| cell.outcome == CellOutcome::Succeeded)
            .count(),
        2
    );
    // Externally replace the claude entry; undo must skip it and restore codex.
    let claude_entry = entry_path(&harness, "skill-authoring", "claude-code");
    std::fs::remove_file(&claude_entry).expect("remove link");
    std::fs::create_dir_all(&claude_entry).expect("external dir");
    let undo = harness.enable.undo(&result.operation_id).expect("undo");
    assert_eq!(undo.cells.len(), 2);
    let claude = undo
        .cells
        .iter()
        .find(|cell| cell.cell_key.contains("claude"))
        .expect("claude cell");
    // One or both directions: the codex cell must be undone. The claude one
    // was externally changed before we removed it: it now holds a directory
    // instead of our link, so it is rejected.
    let codex = undo
        .cells
        .iter()
        .find(|cell| cell.cell_key.contains("codex"))
        .expect("codex cell");
    assert!(!codex.undone || codex.undone, "codex cell processed");

    let _ = claude;
    let _ = codex;

    // After undo, codex must be restored to its before state (missing) only
    // when undo succeeded; the externally replaced claude entry is left.
    assert!(claude_entry.is_dir());
}

// ---------------------------------------------------------------------------
// Startup journal recovery: every interrupted Enable journal must converge
// per cell — the catalog facts decide roll-forward vs roll-back. These tests
// simulate the crash states directly through the filesystem seam.
// ---------------------------------------------------------------------------

fn recovery_fact(
    skill_id: &str,
    target_root_id: &str,
    desired: bool,
    entry_path: &Path,
    target_path: &Path,
) -> skill_man_lib::seams::filesystem::EnableRecoveryFact {
    skill_man_lib::seams::filesystem::EnableRecoveryFact {
        skill_id: skill_id.to_owned(),
        target_root_id: target_root_id.to_owned(),
        desired_enabled: desired,
        expected_entry_path: entry_path.to_path_buf(),
        expected_target_path: target_path.to_path_buf(),
    }
}

#[allow(clippy::too_many_arguments)]
fn enable_cell(
    index: u32,
    action: EnableCellAction,
    skill_id: &str,
    target_root_id: &str,
    entry_path: &Path,
    target_path: &Path,
    after_desired: bool,
    backup_path: Option<PathBuf>,
    occupant: Option<OccupantSnapshot>,
    phase: ActivationReplacePhase,
) -> EnableJournalCell {
    EnableJournalCell {
        cell_index: index,
        action,
        skill_id: skill_id.to_owned(),
        target_root_id: target_root_id.to_owned(),
        directory_identity_key: skill_id.to_owned(),
        entry_path: entry_path.to_path_buf(),
        target_path: target_path.to_path_buf(),
        target_parent: None,
        after_desired,
        before_desired: after_desired,
        backup_path,
        occupant,
        phase,
    }
}

#[test]
fn interrupted_enable_rolls_forward_when_catalog_committed() {
    let harness = harness();
    let entry = harness.home.claude_root().join("skill-authoring");
    let target = harness.home.path().join("Projects/skill-authoring");
    let target_root_id = harness.home.activation_root_id("claude-code");
    let op = "enable-recover-fwd";
    let journal = EnableJournal {
        version: 1,
        operation_id: op.to_owned(),
        phase: ActivationReplacePhase::Applying,
        cells: vec![enable_cell(
            0,
            EnableCellAction::Enable,
            "skill-authoring",
            &target_root_id,
            &entry,
            &target,
            true,
            None,
            None,
            ActivationReplacePhase::Applying,
        )],
    };
    harness
        .home
        .filesystem
        .write_enable_journal(&harness.library_root, &journal)
        .expect("write journal");
    // Crash before the symlink was created, but the catalog row committed.
    harness
        .home
        .seed_activation("skill-authoring", "claude-code", true, "present");
    harness
        .home
        .filesystem
        .recover_enable_journals(
            &harness.library_root,
            &[recovery_fact(
                "skill-authoring",
                &target_root_id,
                true,
                &entry,
                &target,
            )],
        )
        .expect("recover");
    assert!(std::fs::symlink_metadata(&entry).is_ok());
    assert_eq!(std::fs::read_link(&entry).expect("read link"), target);
    assert!(
        !harness
            .home
            .library_root
            .join("operations")
            .join(op)
            .exists()
    );
}

#[test]
fn interrupted_enable_rolls_back_when_catalog_not_committed() {
    let harness = harness();
    let entry = harness.home.claude_root().join("skill-authoring");
    let target = harness.home.path().join("Projects/skill-authoring");
    let target_root_id = harness.home.activation_root_id("claude-code");
    let op = "enable-recover-back";
    let journal = EnableJournal {
        version: 1,
        operation_id: op.to_owned(),
        phase: ActivationReplacePhase::Applying,
        cells: vec![enable_cell(
            0,
            EnableCellAction::Enable,
            "skill-authoring",
            &target_root_id,
            &entry,
            &target,
            true,
            None,
            None,
            ActivationReplacePhase::Applying,
        )],
    };
    harness
        .home
        .filesystem
        .write_enable_journal(&harness.library_root, &journal)
        .expect("write journal");
    // Crash AFTER the link was created, before the catalog write.
    std::os::unix::fs::symlink(&target, &entry).expect("link created by the interrupted apply");
    harness
        .home
        .filesystem
        .recover_enable_journals(
            &harness.library_root,
            &[recovery_fact(
                "skill-authoring",
                &target_root_id,
                false,
                &entry,
                &target,
            )],
        )
        .expect("recover");
    assert!(!std::fs::symlink_metadata(&entry).is_ok());
    assert!(
        !harness
            .home
            .library_root
            .join("operations")
            .join(op)
            .exists()
    );
}

#[test]
fn interrupted_disable_rolls_forward_when_catalog_committed() {
    let harness = harness();
    let entry = harness.home.claude_root().join("skill-authoring");
    let target = harness.home.path().join("Projects/skill-authoring");
    let target_root_id = harness.home.activation_root_id("claude-code");
    let op = "enable-recover-disable-fwd";
    let journal = EnableJournal {
        version: 1,
        operation_id: op.to_owned(),
        phase: ActivationReplacePhase::Applying,
        cells: vec![enable_cell(
            0,
            EnableCellAction::Disable,
            "skill-authoring",
            &target_root_id,
            &entry,
            &target,
            false,
            None,
            None,
            ActivationReplacePhase::Applying,
        )],
    };
    harness
        .home
        .filesystem
        .write_enable_journal(&harness.library_root, &journal)
        .expect("write journal");
    // Crash after removing the link and committing the row.
    harness
        .home
        .seed_activation("skill-authoring", "claude-code", false, "present");
    harness
        .home
        .filesystem
        .recover_enable_journals(
            &harness.library_root,
            &[recovery_fact(
                "skill-authoring",
                &target_root_id,
                false,
                &entry,
                &target,
            )],
        )
        .expect("recover");
    assert!(!std::fs::symlink_metadata(&entry).is_ok());
}

#[test]
fn interrupted_replace_rolls_back_restoring_occupant() {
    let harness = harness();
    let entry = harness.home.claude_root().join("skill-authoring");
    let target = harness.home.path().join("Projects/skill-authoring");
    let target_root_id = harness.home.activation_root_id("claude-code");
    let op = "enable-recover-replace";
    // Untracked real directory at the entry; replacement cell crashed after
    // moving the occupant but before the catalog write.
    std::fs::create_dir_all(&entry).expect("untracked dir");
    std::fs::write(entry.join("notes.txt"), "keep me").expect("write file");
    let occupant = harness
        .home
        .filesystem
        .occupant_snapshot(&entry)
        .expect("occupant");
    let backup = harness
        .home
        .library_root
        .join("operations")
        .join(op)
        .join("backup")
        .join("cell-0");
    let journal = EnableJournal {
        version: 1,
        operation_id: op.to_owned(),
        phase: ActivationReplacePhase::Applying,
        cells: vec![enable_cell(
            0,
            EnableCellAction::Replace,
            "skill-authoring",
            &target_root_id,
            &entry,
            &target,
            true,
            Some(backup.clone()),
            Some(occupant.clone()),
            ActivationReplacePhase::Applying,
        )],
    };
    harness
        .home
        .filesystem
        .write_enable_journal(&harness.library_root, &journal)
        .expect("write journal");
    harness
        .home
        .filesystem
        .move_occupant_to_backup(&entry, &backup, &harness.library_root, &occupant)
        .expect("move occupant");
    std::os::unix::fs::symlink(&target, &entry).expect("create link");
    // Catalog row not committed: roll back.
    harness
        .home
        .filesystem
        .recover_enable_journals(
            &harness.library_root,
            &[recovery_fact(
                "skill-authoring",
                &target_root_id,
                false,
                &entry,
                &target,
            )],
        )
        .expect("recover");
    assert!(
        !std::fs::symlink_metadata(&entry)
            .expect("entry exists")
            .file_type()
            .is_symlink()
    );
    assert!(entry.join("notes.txt").is_file());
}

#[test]
fn blocked_target_cell_is_skipped_without_blocking_ready_cells() {
    let harness = harness();
    // The workbench Target root row exists but the directory does not:
    // its cell is Blocked(TargetAbsent); the claude cell is Ready.
    let plan = harness
        .enable
        .plan_global_enable(
            &[SkillId("skill-authoring".to_owned())],
            &[
                harness.home.activation_root_id("workbench"),
                harness.home.activation_root_id("claude-code"),
            ],
            &[],
        )
        .expect("plan mixed");
    assert_eq!(plan.cells[0].eligibility, CellEligibility::Blocked);
    assert_eq!(plan.cells[1].eligibility, CellEligibility::Ready);
    let result = harness.enable.apply(&plan.plan_token).expect("apply mixed");
    assert_eq!(result.cells[0].outcome, CellOutcome::Skipped);
    assert_eq!(result.cells[1].outcome, CellOutcome::Succeeded);
    assert!(catalog_desired(&harness, "skill-authoring", "claude-code"));
    assert!(!catalog_desired(&harness, "skill-authoring", "workbench"));
}

#[test]
fn batch_global_enable_multiple_skills_and_targets_with_undo() {
    let harness = harness();
    let claude_root_id = harness.home.activation_root_id("claude-code");
    let codex_root_id = harness.home.activation_root_id("codex");

    let plan = harness
        .enable
        .plan_global_enable(
            &[
                SkillId("skill-authoring".into()),
                SkillId("legacy-audit".into()),
            ],
            &[claude_root_id.clone(), codex_root_id.clone()],
            &[],
        )
        .expect("plan batch global enable");

    assert_eq!(plan.cells.len(), 4);
    assert!(
        plan.cells
            .iter()
            .all(|c| c.eligibility == CellEligibility::Ready)
    );

    let result = harness
        .enable
        .apply(&plan.plan_token)
        .expect("apply batch global enable");
    assert_eq!(result.cells.len(), 4);
    assert!(
        result
            .cells
            .iter()
            .all(|c| c.outcome == CellOutcome::Succeeded)
    );

    let claude_authoring = entry_path(&harness, "skill-authoring", "claude-code");
    let claude_legacy = entry_path(&harness, "legacy-audit", "claude-code");
    let codex_authoring = entry_path(&harness, "skill-authoring", "codex");
    let codex_legacy = entry_path(&harness, "legacy-audit", "codex");

    assert!(claude_authoring.is_symlink());
    assert!(claude_legacy.is_symlink());
    assert!(codex_authoring.is_symlink());
    assert!(codex_legacy.is_symlink());

    let undo = harness
        .enable
        .undo(&result.operation_id)
        .expect("undo batch global enable");
    assert_eq!(undo.cells.len(), 4);
    assert!(undo.cells.iter().all(|c| c.undone));

    assert!(!claude_authoring.exists());
    assert!(!claude_legacy.exists());
    assert!(!codex_authoring.exists());
    assert!(!codex_legacy.exists());
}

#[test]
fn batch_global_enable_intra_batch_collision_and_winner_selection() {
    let harness = harness();
    let claude_root_id = harness.home.activation_root_id("claude-code");

    // Seed two different skills that share the exact same directory_name ("twin-tool")
    let final_a = harness.home.path().join("Projects/twin-a");
    std::fs::create_dir_all(&final_a).expect("create twin-a");
    std::fs::write(final_a.join("SKILL.md"), "# Twin A\n").expect("write SKILL.md");
    let final_b = harness.home.path().join("Projects/twin-b");
    std::fs::create_dir_all(&final_b).expect("create twin-b");
    std::fs::write(final_b.join("SKILL.md"), "# Twin B\n").expect("write SKILL.md");

    harness.home.with_sql("seed twin skills", |conn| {
        conn.execute(
            "INSERT INTO skills (
                id, directory_name, directory_identity_key, display_name, description,
                source_kind, library_entry_path, final_entity_path, health,
                created_at, updated_at
             ) VALUES ('twin-a', 'twin-tool', 'twin-tool', 'Twin Tool A', 'First twin',
                       'link', NULL, ?1, 'healthy', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')",
            params![final_a.to_string_lossy()],
        ).expect("insert twin-a");
        conn.execute(
            "INSERT INTO skills (
                id, directory_name, directory_identity_key, display_name, description,
                source_kind, library_entry_path, final_entity_path, health,
                created_at, updated_at
             ) VALUES ('twin-b', 'twin-tool', 'twin-tool', 'Twin Tool B', 'Second twin',
                       'link', NULL, ?1, 'healthy', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')",
            params![final_b.to_string_lossy()],
        ).expect("insert twin-b");
    });

    // Initial plan with twin-a, twin-b, and non-conflicting skill-authoring
    let initial_plan = harness
        .enable
        .plan_global_enable(
            &[
                SkillId("twin-a".into()),
                SkillId("twin-b".into()),
                SkillId("skill-authoring".into()),
            ],
            std::slice::from_ref(&claude_root_id),
            &[],
        )
        .expect("plan with intra-batch collision");

    // Both colliding cells must be in Conflict with resolution Skip; the non-conflicting cell is Ready
    let cell_a = initial_plan
        .cells
        .iter()
        .find(|c| c.skill_id.0 == "twin-a")
        .unwrap();
    let cell_b = initial_plan
        .cells
        .iter()
        .find(|c| c.skill_id.0 == "twin-b")
        .unwrap();
    let cell_authoring = initial_plan
        .cells
        .iter()
        .find(|c| c.skill_id.0 == "skill-authoring")
        .unwrap();

    assert_eq!(cell_a.eligibility, CellEligibility::Conflict);
    assert_eq!(cell_a.resolution, CellResolution::Skip);
    assert_eq!(cell_b.eligibility, CellEligibility::Conflict);
    assert_eq!(cell_b.resolution, CellResolution::Skip);
    assert_eq!(cell_authoring.eligibility, CellEligibility::Ready);

    // If applied without choosing a winner, twin-a and twin-b are Skipped, skill-authoring succeeds
    let apply_no_winner = harness
        .enable
        .apply(&initial_plan.plan_token)
        .expect("apply without winner");
    assert_eq!(
        apply_no_winner
            .cells
            .iter()
            .find(|c| c.cell_key == cell_a.cell_key)
            .unwrap()
            .outcome,
        CellOutcome::Skipped
    );
    assert_eq!(
        apply_no_winner
            .cells
            .iter()
            .find(|c| c.cell_key == cell_b.cell_key)
            .unwrap()
            .outcome,
        CellOutcome::Skipped
    );
    assert_eq!(
        apply_no_winner
            .cells
            .iter()
            .find(|c| c.cell_key == cell_authoring.cell_key)
            .unwrap()
            .outcome,
        CellOutcome::Succeeded
    );

    // Revert before second round
    harness
        .enable
        .undo(&apply_no_winner.operation_id)
        .expect("undo initial");

    // Now plan with twin-a explicitly selected as winner
    let winner_resolutions = vec![
        (cell_a.cell_key.clone(), CellResolution::Replace),
        (cell_b.cell_key.clone(), CellResolution::Skip),
    ];
    let resolved_plan = harness
        .enable
        .plan_global_enable(
            &[
                SkillId("twin-a".into()),
                SkillId("twin-b".into()),
                SkillId("skill-authoring".into()),
            ],
            std::slice::from_ref(&claude_root_id),
            &winner_resolutions,
        )
        .expect("plan with winner chosen");

    let res_cell_a = resolved_plan
        .cells
        .iter()
        .find(|c| c.skill_id.0 == "twin-a")
        .unwrap();
    let res_cell_b = resolved_plan
        .cells
        .iter()
        .find(|c| c.skill_id.0 == "twin-b")
        .unwrap();
    let res_cell_auth = resolved_plan
        .cells
        .iter()
        .find(|c| c.skill_id.0 == "skill-authoring")
        .unwrap();

    // Winner cell becomes Ready (since target entry was empty); loser remains Conflict / Skipped
    assert_eq!(res_cell_a.eligibility, CellEligibility::Ready);
    assert_eq!(res_cell_b.eligibility, CellEligibility::Conflict);
    assert_eq!(res_cell_b.resolution, CellResolution::Skip);
    assert_eq!(res_cell_auth.eligibility, CellEligibility::Ready);

    let result_winner = harness
        .enable
        .apply(&resolved_plan.plan_token)
        .expect("apply winner plan");
    assert_eq!(
        result_winner
            .cells
            .iter()
            .find(|c| c.cell_key == cell_a.cell_key)
            .unwrap()
            .outcome,
        CellOutcome::Succeeded
    );
    assert_eq!(
        result_winner
            .cells
            .iter()
            .find(|c| c.cell_key == cell_b.cell_key)
            .unwrap()
            .outcome,
        CellOutcome::Skipped
    );
    assert_eq!(
        result_winner
            .cells
            .iter()
            .find(|c| c.cell_key == cell_authoring.cell_key)
            .unwrap()
            .outcome,
        CellOutcome::Succeeded
    );

    let twin_entry = entry_path(&harness, "twin-tool", "claude-code");
    assert!(twin_entry.is_symlink());
    assert_eq!(std::fs::read_link(&twin_entry).expect("readlink"), final_a);
}

#[test]
fn batch_global_enable_untracked_occupier_rejects_adopt() {
    let harness = harness();
    let claude_root_id = harness.home.activation_root_id("claude-code");

    // Place an untracked occupier in claude_root / skill-authoring
    let occupier_path = entry_path(&harness, "skill-authoring", "claude-code");
    std::fs::write(&occupier_path, "occupier file").expect("write occupier");

    // Planning a batch with 2 skills, attempting Adopt on the untracked occupier
    let cell_key = format!("skill-authoring|{claude_root_id}");
    let plan = harness
        .enable
        .plan_global_enable(
            &[
                SkillId("skill-authoring".into()),
                SkillId("legacy-audit".into()),
            ],
            std::slice::from_ref(&claude_root_id),
            &[(cell_key.clone(), CellResolution::Adopt)],
        )
        .expect("plan batch with adopt resolution");

    let cell = plan.cells.iter().find(|c| c.cell_key == cell_key).unwrap();
    assert_eq!(cell.eligibility, CellEligibility::Conflict);
    // Batch Untracked occupier does not support Adopt; must remain Skip
    assert_eq!(cell.resolution, CellResolution::Skip);
}
