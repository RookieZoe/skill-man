//! Project Enable vertical slice (spec §4.9; ADR-0015; ADR-0019; #89):
//! Project root canonicalization, bounded symlink walker with strict
//! containment, temporary resolved-target group merging ("1 physical write"),
//! exact-direct link no-op, other symlink and real directory replacement,
//! MRU update only on success, zero catalog activation persistence,
//! and crash-safe undo/finalize window.

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::params;
use skill_man_lib::adapters::agent_configuration_fs::MacOsAgentConfigurationFileSystem;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::domain::SkillId;
use skill_man_lib::core::enable::{
    CellBlockedReason, CellEligibility, CellOutcome, CellResolution, EnableService,
};
use skill_man_lib::seams::activation_store::ActivationStore;
use skill_man_lib::seams::catalog_store::CatalogStore;

mod common;
use common::BoundTestHome;

struct Harness {
    home: BoundTestHome,
    enable: EnableService,
}

#[test]
fn hop_limit_exceeded_blocks_project_enable() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude"));

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();

    // Create a chain of 17 symlinks: hop_0 -> hop_1 -> ... -> hop_17 -> final_dir
    let final_dir = project_root.join("final_skills");
    std::fs::create_dir_all(&final_dir).expect("create final dir");

    let mut prev = "final_skills".to_string();
    for i in (0..=17).rev() {
        let link = project_root.join(format!("hop_{i}"));
        std::os::unix::fs::symlink(&prev, &link).expect("symlink");
        prev = format!("hop_{i}");
    }
    let claude_link = project_root.join(".claude");
    std::os::unix::fs::symlink(&prev, &claude_link).expect("claude symlink");

    let skill_id = SkillId("skill-authoring".into());
    let plan = harness
        .enable
        .plan_project_enable(&[skill_id], &project_root, &["claude-code".into()], &[])
        .expect("plan hop limit");

    assert_eq!(plan.cells[0].eligibility, CellEligibility::Blocked);
    assert_eq!(
        plan.cells[0].blocked_reason,
        Some(CellBlockedReason::HopLimitExceeded)
    );
}

#[test]
fn target_not_directory_blocks_project_enable() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();

    // Create .claude as a regular file
    std::fs::write(project_root.join(".claude"), "not a directory").expect("write file");

    let skill_id = SkillId("skill-authoring".into());
    let plan = harness
        .enable
        .plan_project_enable(&[skill_id], &project_root, &["claude-code".into()], &[])
        .expect("plan");

    assert_eq!(plan.cells[0].eligibility, CellEligibility::Blocked);
    assert_eq!(
        plan.cells[0].blocked_reason,
        Some(CellBlockedReason::TargetNotDirectory)
    );
}

#[test]
fn source_snapshot_mismatch_blocks_new_project_enable() {
    let home = BoundTestHome::new();
    home.seed_standard_library();

    // Seed mismatched git member
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
        connection
            .execute(
                "UPDATE agent_configurations SET project_skills_dir = '.claude/skills' WHERE agent_id = 'claude-code'",
                [],
            )
            .expect("update agent");
    });

    let library_root = home.library_root.clone();
    let source: Arc<dyn skill_man_lib::seams::source::GitSource> =
        Arc::new(skill_man_lib::adapters::git_source::SystemGitSource::new());
    let locks = Arc::new(
        skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore::new(
            home.path().to_path_buf(),
        ),
    );
    let preview = Arc::new(
        skill_man_lib::core::source_group_preview::SourceGroupPreviewService::new(
            source.clone(),
            locks.clone(),
        ),
    );
    let transition = Arc::new(
        skill_man_lib::core::source_transition::SourceTransitionService::new(
            preview.clone(),
            source.clone(),
            locks.clone(),
            home.runtime.clone(),
            home.filesystem.clone(),
            Arc::new(SystemClock::new()),
            library_root.clone(),
            home.path().to_path_buf(),
        )
        .with_update_store(home.runtime.clone()),
    );
    let source_update = Arc::new(
        skill_man_lib::core::source_update::SourceUpdateService::new(
            preview.clone(),
            transition.clone(),
            home.filesystem.clone(),
            library_root.clone(),
        )
        .with_home_context(home.write_gate.clone()),
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
        library_root,
    )
    .with_write_gate(home.write_gate.clone())
    .with_home_context(home.write_gate.clone())
    .with_source_update(source_update);

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let plan = enable
        .plan_project_enable(
            &[SkillId("git-skill".into())],
            temp_proj.path(),
            &["claude-code".into()],
            &[],
        )
        .expect("plan");
    assert_eq!(plan.cells[0].eligibility, CellEligibility::Blocked);
    assert_eq!(
        plan.cells[0].blocked_reason,
        Some(CellBlockedReason::SourceSnapshotMismatch)
    );
}

fn harness() -> Harness {
    let home = BoundTestHome::new();
    home.seed_standard_library();
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
    )
    .with_write_gate(home.write_gate.clone())
    .with_home_context(home.write_gate.clone());
    Harness { home, enable }
}

fn set_agent_project_skills_dir(harness: &Harness, agent_id: &str, dir: Option<&str>) {
    harness
        .home
        .with_sql("set project_skills_dir", |connection| {
            connection
                .execute(
                    "UPDATE agent_configurations SET project_skills_dir = ?1 WHERE agent_id = ?2",
                    params![dir, agent_id],
                )
                .expect("update agent project_skills_dir");
        });
}

#[test]
fn plan_project_enable_with_create_steps_and_apply_undo_finalize() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));

    // Create an isolated project directory
    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();

    let skill_id = SkillId("skill-authoring".into());

    // 1. Plan Project Enable: target container `.claude/skills` does not exist yet
    let plan = harness
        .enable
        .plan_project_enable(
            std::slice::from_ref(&skill_id),
            &project_root,
            &["claude-code".into()],
            &[],
        )
        .expect("plan project enable");

    assert_eq!(plan.scope, "project");
    assert!(plan.project_root.is_some());
    let proj_evidence = plan.project_root.as_ref().unwrap();
    assert_eq!(
        proj_evidence.canonical_path,
        std::fs::canonicalize(&project_root).unwrap()
    );

    assert_eq!(plan.cells.len(), 1);
    let cell = &plan.cells[0];
    assert_eq!(cell.skill_id, skill_id);
    assert_eq!(cell.eligibility, CellEligibility::Ready);
    assert_eq!(cell.affected_agent_ids, vec!["claude-code"]);
    assert_eq!(cell.affected_agent_names, vec!["Claude Code"]);
    assert!(
        !cell.create_steps.is_empty(),
        "should report create steps for missing container"
    );

    // 2. Apply: container and symlink are created
    let result = harness
        .enable
        .apply(&plan.plan_token)
        .expect("apply project enable");
    assert_eq!(result.cells.len(), 1);
    assert_eq!(result.cells[0].outcome, CellOutcome::Succeeded);

    // Verify entry exists and points to final entity
    let expected_entry = project_root.join(".claude/skills/skill-authoring");
    assert!(expected_entry.is_symlink());
    let target = std::fs::read_link(&expected_entry).expect("read symlink");
    let skill = harness
        .home
        .runtime
        .inspect(&skill_id)
        .expect("inspect")
        .unwrap();
    assert_eq!(target, PathBuf::from(skill.final_entity_path));

    // Invariant: Project entry/group NEVER enters Catalog activations
    let activations = harness
        .home
        .runtime
        .activation_cells_for_skill(&skill_id)
        .expect("activations");
    assert!(
        activations.is_empty(),
        "project enable must never write to catalog activations table"
    );

    // Invariant: At least one cell succeeded -> recent_project_folders updated
    let recent = harness
        .enable
        .list_recent_project_folders()
        .expect("list recent project folders");
    assert_eq!(recent.len(), 1);
    assert_eq!(
        recent[0].canonical_path,
        std::fs::canonicalize(&project_root).unwrap()
    );

    // 3. Undo: removes symlink
    let undo_result = harness
        .enable
        .undo(&result.operation_id)
        .expect("undo project enable");
    assert_eq!(undo_result.cells.len(), 1);
    assert!(undo_result.cells[0].undone);
    assert!(!expected_entry.exists());
}

#[test]
fn plan_project_enable_finalize_closes_undo_window() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));
    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();
    let skill_id = SkillId("skill-authoring".into());

    let plan = harness
        .enable
        .plan_project_enable(
            std::slice::from_ref(&skill_id),
            &project_root,
            &["claude-code".into()],
            &[],
        )
        .expect("plan");
    let result = harness.enable.apply(&plan.plan_token).expect("apply");

    // Finalize closes undo window
    harness
        .enable
        .finalize(&result.operation_id)
        .expect("finalize");
    assert!(harness.enable.finalize(&result.operation_id).is_err());
}

#[test]
fn plan_project_enable_merges_multiple_agents_resolving_to_same_container() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));
    set_agent_project_skills_dir(&harness, "codex", Some(".agents/skills"));

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();

    // Create .claude/skills directory
    let claude_skills = project_root.join(".claude/skills");
    std::fs::create_dir_all(&claude_skills).expect("create claude skills");

    // In project, .agents is a symlink pointing to .claude
    let agents_link = project_root.join(".agents");
    std::os::unix::fs::symlink(".claude", &agents_link).expect("create agents symlink");

    let skill_id = SkillId("skill-authoring".into());

    let plan = harness
        .enable
        .plan_project_enable(
            std::slice::from_ref(&skill_id),
            &project_root,
            &["claude-code".into(), "codex".into()],
            &[],
        )
        .expect("plan project enable");

    // Both agents resolve to the same container: merged into 1 cell ("1 physical write")
    assert_eq!(plan.cells.len(), 1);
    let cell = &plan.cells[0];
    assert_eq!(cell.affected_agent_ids.len(), 2);
    assert!(cell.affected_agent_ids.contains(&"claude-code".to_string()));
    assert!(cell.affected_agent_ids.contains(&"codex".to_string()));
    assert_eq!(
        cell.target_path,
        std::fs::canonicalize(&claude_skills).unwrap()
    );

    // Apply writes only once
    let result = harness.enable.apply(&plan.plan_token).expect("apply");
    assert_eq!(result.cells.len(), 1);
    assert_eq!(result.cells[0].outcome, CellOutcome::Succeeded);
}

#[test]
fn exact_direct_link_is_noop_and_does_not_update_recent_folders() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();
    let claude_skills = project_root.join(".claude/skills");
    std::fs::create_dir_all(&claude_skills).expect("create claude skills");

    let skill_id = SkillId("skill-authoring".into());
    let skill = harness
        .home
        .runtime
        .inspect(&skill_id)
        .expect("inspect")
        .unwrap();

    // Pre-create exact direct link
    let entry = claude_skills.join("skill-authoring");
    std::os::unix::fs::symlink(&skill.final_entity_path, &entry).expect("symlink");

    let plan = harness
        .enable
        .plan_project_enable(
            std::slice::from_ref(&skill_id),
            &project_root,
            &["claude-code".into()],
            &[],
        )
        .expect("plan project enable");

    assert_eq!(plan.cells.len(), 1);
    let cell = &plan.cells[0];
    assert_eq!(cell.eligibility, CellEligibility::NoOp);
    assert!(cell.occ_exact_direct);

    let result = harness.enable.apply(&plan.plan_token).expect("apply");
    assert_eq!(result.cells[0].outcome, CellOutcome::NoOp);

    // AC: 0 cells succeeded -> recent_project_folders is NOT updated
    let recent = harness
        .enable
        .list_recent_project_folders()
        .expect("list recent project folders");
    assert!(
        recent.is_empty(),
        "zero successful cells must not update recent_project_folders"
    );
}

#[test]
fn replace_real_directory_with_destructive_counts_and_undo() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();
    let claude_skills = project_root.join(".claude/skills");
    std::fs::create_dir_all(&claude_skills).expect("create claude skills");

    let skill_id = SkillId("skill-authoring".into());
    let entry = claude_skills.join("skill-authoring");

    // Occupy with real directory containing 2 files and 1 subdir
    std::fs::create_dir_all(entry.join("sub")).expect("create subdir");
    std::fs::write(entry.join("file1.txt"), "hello").expect("write file1");
    std::fs::write(entry.join("sub/file2.txt"), "world").expect("write file2");

    // Initial plan without resolution: Conflict
    let plan = harness
        .enable
        .plan_project_enable(
            std::slice::from_ref(&skill_id),
            &project_root,
            &["claude-code".into()],
            &[],
        )
        .expect("plan project enable");

    assert_eq!(plan.cells[0].eligibility, CellEligibility::Conflict);
    assert_eq!(plan.cells[0].resolution, CellResolution::Skip);
    let destructive = plan.cells[0]
        .destructive
        .as_ref()
        .expect("destructive counts");
    assert_eq!(destructive.directories, 1);
    assert_eq!(destructive.files, 2);

    // Plan with resolution: Replace
    let cell_key = plan.cells[0].cell_key.clone();
    let resolved_plan = harness
        .enable
        .plan_project_enable(
            std::slice::from_ref(&skill_id),
            &project_root,
            &["claude-code".into()],
            &[(cell_key, CellResolution::Replace)],
        )
        .expect("plan project enable with replace");

    assert_eq!(
        resolved_plan.cells[0].eligibility,
        CellEligibility::Conflict
    );
    assert_eq!(resolved_plan.cells[0].resolution, CellResolution::Replace);

    // Apply: backs up real directory and creates symlink
    let result = harness
        .enable
        .apply(&resolved_plan.plan_token)
        .expect("apply replace");
    assert_eq!(result.cells[0].outcome, CellOutcome::Succeeded);
    assert!(entry.is_symlink());

    // Undo: restores real directory and its contents
    let undo_result = harness
        .enable
        .undo(&result.operation_id)
        .expect("undo replace");
    assert!(undo_result.cells[0].undone);
    assert!(entry.is_dir());
    assert!(!entry.is_symlink());
    assert_eq!(
        std::fs::read_to_string(entry.join("file1.txt")).unwrap(),
        "hello"
    );
    assert_eq!(
        std::fs::read_to_string(entry.join("sub/file2.txt")).unwrap(),
        "world"
    );
}

#[test]
fn containment_violations_block_cell_without_override() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();

    // 1. Outside project root: .claude is a symlink pointing outside
    let outside_dir = tempfile::tempdir().expect("outside temp dir");
    let claude_link = project_root.join(".claude");
    std::os::unix::fs::symlink(outside_dir.path(), &claude_link).expect("symlink outside");

    let skill_id = SkillId("skill-authoring".into());
    let plan = harness
        .enable
        .plan_project_enable(
            std::slice::from_ref(&skill_id),
            &project_root,
            &["claude-code".into()],
            &[],
        )
        .expect("plan");

    assert_eq!(plan.cells[0].eligibility, CellEligibility::Blocked);
    assert_eq!(
        plan.cells[0].blocked_reason,
        Some(CellBlockedReason::OutsideProjectRoot)
    );

    // 2. Symlink cycle
    let temp_proj2 = tempfile::tempdir().expect("temp project dir 2");
    let project_root2 = temp_proj2.path().to_path_buf();
    let loop_a = project_root2.join(".claude");
    let loop_b = project_root2.join(".loop_b");
    std::os::unix::fs::symlink(".loop_b", &loop_a).expect("loop a");
    std::os::unix::fs::symlink(".claude", &loop_b).expect("loop b");

    let plan2 = harness
        .enable
        .plan_project_enable(
            std::slice::from_ref(&skill_id),
            &project_root2,
            &["claude-code".into()],
            &[],
        )
        .expect("plan loop");

    assert_eq!(plan2.cells[0].eligibility, CellEligibility::Blocked);
    assert_eq!(
        plan2.cells[0].blocked_reason,
        Some(CellBlockedReason::SymlinkCycle)
    );
}

#[test]
fn recent_project_folders_mru_and_clear() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".skills"));
    let skill_id = SkillId("skill-authoring".into());

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();

    let plan = harness
        .enable
        .plan_project_enable(&[skill_id], &project_root, &["claude-code".into()], &[])
        .expect("plan");

    harness.enable.apply(&plan.plan_token).expect("apply");

    let list = harness
        .enable
        .list_recent_project_folders()
        .expect("list recent");
    assert_eq!(list.len(), 1);

    harness
        .enable
        .clear_recent_project_folders()
        .expect("clear recent");
    let cleared = harness
        .enable
        .list_recent_project_folders()
        .expect("list cleared");
    assert!(cleared.is_empty());
}

#[test]
fn batch_project_enable_multiple_skills_and_agents_with_undo() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));
    set_agent_project_skills_dir(&harness, "codex", Some(".codex/skills"));

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();

    let plan = harness
        .enable
        .plan_project_enable(
            &[
                SkillId("skill-authoring".into()),
                SkillId("legacy-audit".into()),
            ],
            &project_root,
            &["claude-code".into(), "codex".into()],
            &[],
        )
        .expect("plan batch project enable");

    // 2 target containers × 2 skills = 4 cells
    assert_eq!(plan.cells.len(), 4);
    assert!(
        plan.cells
            .iter()
            .all(|c| c.eligibility == CellEligibility::Ready)
    );

    let result = harness
        .enable
        .apply(&plan.plan_token)
        .expect("apply batch project enable");
    assert_eq!(result.cells.len(), 4);
    assert!(
        result
            .cells
            .iter()
            .all(|c| c.outcome == CellOutcome::Succeeded)
    );

    let claude_authoring = project_root.join(".claude/skills/skill-authoring");
    let claude_legacy = project_root.join(".claude/skills/legacy-audit");
    let codex_authoring = project_root.join(".codex/skills/skill-authoring");
    let codex_legacy = project_root.join(".codex/skills/legacy-audit");

    assert!(claude_authoring.is_symlink());
    assert!(claude_legacy.is_symlink());
    assert!(codex_authoring.is_symlink());
    assert!(codex_legacy.is_symlink());

    // Undo reverts all 4 project symlinks
    let undo = harness
        .enable
        .undo(&result.operation_id)
        .expect("undo batch project enable");
    assert_eq!(undo.cells.len(), 4);
    assert!(undo.cells.iter().all(|c| c.undone));

    assert!(!claude_authoring.exists());
    assert!(!claude_legacy.exists());
    assert!(!codex_authoring.exists());
    assert!(!codex_legacy.exists());
}

#[test]
fn batch_project_enable_intra_batch_collision_and_winner_selection() {
    let harness = harness();
    set_agent_project_skills_dir(&harness, "claude-code", Some(".claude/skills"));

    let final_a = harness.home.path().join("Projects/twin-a");
    std::fs::create_dir_all(&final_a).expect("create twin-a");
    std::fs::write(final_a.join("SKILL.md"), "# Twin A\n").expect("write SKILL.md");
    let final_b = harness.home.path().join("Projects/twin-b");
    std::fs::create_dir_all(&final_b).expect("create twin-b");
    std::fs::write(final_b.join("SKILL.md"), "# Twin B\n").expect("write SKILL.md");

    harness.home.with_sql("seed twin skills for project", |conn| {
        conn.execute(
            "INSERT INTO skills (
                id, directory_name, directory_identity_key, display_name, description,
                source_kind, library_entry_path, final_entity_path, health,
                created_at, updated_at
             ) VALUES ('twin-proj-a', 'twin-tool', 'twin-tool', 'Twin Tool A', 'First twin',
                       'link', NULL, ?1, 'healthy', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')",
            params![final_a.to_string_lossy()],
        ).expect("insert twin-proj-a");
        conn.execute(
            "INSERT INTO skills (
                id, directory_name, directory_identity_key, display_name, description,
                source_kind, library_entry_path, final_entity_path, health,
                created_at, updated_at
             ) VALUES ('twin-proj-b', 'twin-tool', 'twin-tool', 'Twin Tool B', 'Second twin',
                       'link', NULL, ?1, 'healthy', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')",
            params![final_b.to_string_lossy()],
        ).expect("insert twin-proj-b");
    });

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let project_root = temp_proj.path().to_path_buf();

    // Initial plan without winner
    let initial_plan = harness
        .enable
        .plan_project_enable(
            &[SkillId("twin-proj-a".into()), SkillId("twin-proj-b".into())],
            &project_root,
            &["claude-code".into()],
            &[],
        )
        .expect("plan collision");

    assert_eq!(initial_plan.cells.len(), 2);
    assert_eq!(initial_plan.cells[0].eligibility, CellEligibility::Conflict);
    assert_eq!(initial_plan.cells[0].resolution, CellResolution::Skip);
    assert_eq!(initial_plan.cells[1].eligibility, CellEligibility::Conflict);
    assert_eq!(initial_plan.cells[1].resolution, CellResolution::Skip);

    // Re-plan with twin-proj-a chosen as winner
    let winner_key = initial_plan.cells[0].cell_key.clone();
    let winner_plan = harness
        .enable
        .plan_project_enable(
            &[SkillId("twin-proj-a".into()), SkillId("twin-proj-b".into())],
            &project_root,
            &["claude-code".into()],
            &[(winner_key, CellResolution::Replace)],
        )
        .expect("plan with winner");

    assert_eq!(winner_plan.cells[0].eligibility, CellEligibility::Ready);
    assert_eq!(winner_plan.cells[1].eligibility, CellEligibility::Conflict);
    assert_eq!(winner_plan.cells[1].resolution, CellResolution::Skip);
}
