//! Project source-health gate regression; copy lifecycle coverage uses EnableApi
//! in project_skill_copy.rs. ADR-0025 supersedes the former direct-link tests.
use rusqlite::params;
use skill_man_lib::adapters::agent_configuration_fs::MacOsAgentConfigurationFileSystem;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::domain::SkillId;
use skill_man_lib::core::enable::{CellBlockedReason, CellEligibility, EnableService};
use std::sync::Arc;
mod common;
use common::BoundTestHome;
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
            home.write_gate.clone(),
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
        library_root,
        home.write_gate.clone(),
    )
    .with_home_context(home.write_gate.clone())
    .with_source_update(source_update);

    let temp_proj = tempfile::tempdir().expect("temp project dir");
    let plan = enable
        .plan_project_enable(&[SkillId("git-skill".into())], temp_proj.path(), &[], &[])
        .expect("plan");
    assert_eq!(plan.cells[0].eligibility, CellEligibility::Blocked);
    assert_eq!(
        plan.cells[0].blocked_reason,
        Some(CellBlockedReason::SourceSnapshotMismatch)
    );
}
