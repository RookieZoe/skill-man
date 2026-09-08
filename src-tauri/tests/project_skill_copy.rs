use skill_man_lib::adapters::agent_configuration_fs::MacOsAgentConfigurationFileSystem;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::enable::EnableService;
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::tauri_adapter::dto::{ApplyGlobalEnableRequestDto, PlanProjectEnableRequestDto};
use skill_man_lib::tauri_adapter::enable_api::EnableApi;
use std::sync::Arc;
mod common;
use common::BoundTestHome;

#[test]
fn zero_agents_preview_is_read_only_and_apply_delivers_a_real_copy() {
    let (_home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    let global_before =
        serde_json::to_value(api.list_target_groups("skill-authoring".into()).unwrap()).unwrap();
    let plan = api
        .plan_project_enable(PlanProjectEnableRequestDto {
            skill_ids: vec!["skill-authoring".into()],
            project_folder: project.path().to_string_lossy().into_owned(),
            agent_ids: vec![],
            cell_resolutions: vec![],
        })
        .expect("zero Agent plan");
    assert_eq!(plan.cells.len(), 1);

    assert!(!project.path().join(".agents").exists());
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: plan.plan_token,
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(&result).unwrap()["cells"][0]["outcome"],
        "succeeded"
    );
    let copy = project.path().join(".agents/skills/skill-authoring");
    assert!(copy.join("SKILL.md").is_file());
    assert!(
        !std::fs::symlink_metadata(copy)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        global_before["groups"],
        serde_json::to_value(api.list_target_groups("skill-authoring".into()).unwrap()).unwrap()["groups"]
    );
}

fn harness() -> (BoundTestHome, EnableApi) {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let api = EnableApi::new(
        EnableService::new(
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
        .with_home_context(home.write_gate.clone()),
    );
    (home, api)
}

#[test]
fn existing_project_edits_are_reused_and_undo_preserves_them() {
    let (_home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    let copy = project.path().join(".agents/skills/skill-authoring");
    std::fs::create_dir_all(copy.join(".git")).unwrap();
    std::fs::write(copy.join("SKILL.md"), "# Project version\n").unwrap();
    std::fs::write(copy.join(".git/config"), "project metadata").unwrap();
    let plan = plan(&api, &project);
    assert_eq!(
        serde_json::to_value(&plan).unwrap()["cells"][0]["eligibility"],
        "no_op"
    );
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: plan.plan_token,
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(&result).unwrap()["cells"][0]["outcome"],
        "no_op"
    );
    api.undo_project_enable(
        skill_man_lib::tauri_adapter::dto::EnableOperationRequestDto {
            operation_id: result.operation_id,
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(copy.join("SKILL.md")).unwrap(),
        "# Project version\n"
    );
    assert!(copy.join(".git/config").exists());
    assert!(api.list_recent_project_folders().unwrap().is_empty());
}
fn plan(
    api: &EnableApi,
    project: &tempfile::TempDir,
) -> skill_man_lib::tauri_adapter::dto::EnablePlanDto {
    api.plan_project_enable(PlanProjectEnableRequestDto {
        skill_ids: vec!["skill-authoring".into()],
        project_folder: project.path().to_string_lossy().into_owned(),
        agent_ids: vec![],
        cell_resolutions: vec![],
    })
    .unwrap()
}

#[test]
fn new_copy_undo_preserves_external_edits_and_finalize_is_repeatable() {
    use skill_man_lib::tauri_adapter::dto::EnableOperationRequestDto;
    let (_home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    let p = plan(&api, &project);
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: p.plan_token,
        })
        .unwrap();
    let copy = project.path().join(".agents/skills/skill-authoring");
    std::fs::write(copy.join("project.txt"), "keep me").unwrap();
    let undo = api
        .undo_project_enable(EnableOperationRequestDto {
            operation_id: result.operation_id.clone(),
        })
        .unwrap();
    assert!(!undo.cells[0].undone);
    assert_eq!(
        std::fs::read_to_string(copy.join("project.txt")).unwrap(),
        "keep me"
    );
    api.finalize_project_enable(EnableOperationRequestDto {
        operation_id: result.operation_id.clone(),
    })
    .unwrap();
    api.finalize_project_enable(EnableOperationRequestDto {
        operation_id: result.operation_id,
    })
    .unwrap();
}

#[test]
fn delivered_payload_survives_project_move_without_the_source() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    let source = home.path().join("Projects/skill-authoring");
    std::fs::create_dir_all(source.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(source.join(".git")).unwrap();
    std::fs::write(source.join(".git/config"), "excluded").unwrap();
    std::fs::write(source.join(".hidden"), "hidden data").unwrap();
    std::fs::write(
        source.join("node_modules/pkg/run"),
        "#!/bin/sh\necho original\n",
    )
    .unwrap();
    std::fs::set_permissions(
        source.join("node_modules/pkg/run"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink(source.join("node_modules/pkg/run"), source.join("run")).unwrap();
    let p = plan(&api, &project);
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: p.plan_token,
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap()["cells"][0]["outcome"],
        "succeeded"
    );
    let moved = project.path().with_extension("moved");
    std::fs::rename(project.path(), &moved).unwrap();
    std::fs::rename(&source, source.with_extension("isolated")).unwrap();
    let copy = moved.join(".agents/skills/skill-authoring");
    assert_eq!(
        std::fs::read_to_string(copy.join("run")).unwrap(),
        "#!/bin/sh\necho original\n"
    );
    assert_eq!(
        std::fs::read_to_string(copy.join(".hidden")).unwrap(),
        "hidden data"
    );
    assert_eq!(
        std::fs::metadata(copy.join("run"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(!copy.join(".git").exists());
    assert!(!std::fs::read_link(copy.join("run")).unwrap().is_absolute());
    std::fs::remove_dir_all(moved).unwrap();
}

#[test]
fn cycles_through_sibling_directories_and_excluded_link_hops_are_blocked() {
    use std::os::unix::fs::symlink;
    for excluded in [false, true] {
        let (home, api) = harness();
        let project = tempfile::tempdir().unwrap();
        let source = home.path().join("Projects/skill-authoring");
        if excluded {
            std::fs::create_dir(source.join(".git")).unwrap();
            symlink("../SKILL.md", source.join(".git/indirect")).unwrap();
            symlink(".git/indirect", source.join("readme")).unwrap();
        } else {
            std::fs::create_dir(source.join("a")).unwrap();
            std::fs::create_dir(source.join("b")).unwrap();
            symlink("../b", source.join("a/next")).unwrap();
            symlink("../a", source.join("b/next")).unwrap();
        }
        let p = plan(&api, &project);
        assert_eq!(
            serde_json::to_value(p).unwrap()["cells"][0]["eligibility"],
            "blocked"
        );
        assert!(!project.path().join(".agents").exists());
    }
}

#[test]
fn stale_source_or_project_ancestor_never_writes_a_copy() {
    for change_source in [true, false] {
        let (home, api) = harness();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".agents/skills")).unwrap();
        let p = plan(&api, &project);
        if change_source {
            std::fs::write(
                home.path().join("Projects/skill-authoring/SKILL.md"),
                "# Changed source",
            )
            .unwrap();
        } else {
            std::fs::rename(project.path().join(".agents"), project.path().join("saved")).unwrap();
            std::fs::create_dir_all(project.path().join(".agents/skills")).unwrap();
        }
        assert!(
            api.apply_project_enable(ApplyGlobalEnableRequestDto {
                plan_token: p.plan_token
            })
            .is_err()
        );
        assert!(
            !project
                .path()
                .join(".agents/skills/skill-authoring")
                .exists()
        );
    }
}
#[test]
fn occupied_and_non_self_contained_entries_are_preserved() {
    use std::os::unix::fs::symlink;
    for kind in ["file", "invalid", "external", "dangling", "absolute_alias"] {
        let (_home, api) = harness();
        let project = tempfile::tempdir().unwrap();
        let copy = project.path().join(".agents/skills/skill-authoring");
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        let external = tempfile::tempdir().unwrap();
        match kind {
            "file" => std::fs::write(&copy, "keep").unwrap(),
            "invalid" => std::fs::create_dir(&copy).unwrap(),
            "external" => symlink(external.path(), &copy).unwrap(),
            "dangling" => symlink("missing", &copy).unwrap(),
            _ => {
                let inside = project.path().join("inside");
                std::fs::create_dir(&inside).unwrap();
                std::fs::write(inside.join("SKILL.md"), "# Existing").unwrap();
                symlink(inside, &copy).unwrap();
            }
        }
        let before = std::fs::symlink_metadata(&copy).unwrap();
        let p = plan(&api, &project);
        assert_eq!(
            serde_json::to_value(&p).unwrap()["cells"][0]["eligibility"],
            "blocked",
            "{kind}"
        );
        api.apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: p.plan_token,
        })
        .unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            before.ino(),
            std::fs::symlink_metadata(&copy).unwrap().ino()
        );
    }
}

#[test]
fn startup_recovers_copy_phases_without_catalog_activation_or_current_source() {
    use skill_man_lib::core::maintenance::MaintenanceService;
    use skill_man_lib::seams::filesystem::{
        ActivationReplacePhase as Phase, EnableJournal, ProjectCopyJournal,
    };
    use skill_man_lib::tauri_adapter::health_api::HealthApi;
    use std::path::Path;
    for phase in [Phase::Applying, Phase::Committed, Phase::Undoing] {
        let (home, _api) = harness();
        let project = tempfile::tempdir().unwrap();
        let fs = &home.filesystem;
        let root = fs.directory_fingerprint(project.path()).unwrap();
        let resolution = fs
            .resolve_project_target(&root.canonical_path, Path::new(".agents/skills"))
            .unwrap();
        let parent = fs.prepare_project_copy_parent(&root, &resolution).unwrap();
        let mut copy = ProjectCopyJournal {
            cleanup_authorized: false,
            target_resolution: fs
                .resolve_project_target(&root.canonical_path, Path::new(".agents/skills"))
                .unwrap(),
            project_root: root,
            parent: parent.clone(),
            entry_path: parent.canonical_path.join("skill-authoring"),
            staging_path: parent.canonical_path.join(".skillman-crash"),
            staged_identity: None,
            content_hash: None,
            phase,
        };
        let source = home.path().join("Projects/skill-authoring");
        std::fs::write(source.join("SKILL.md"), "# Frozen delivered version\n").unwrap();
        let payload = fs.project_copy_payload(&source, false).unwrap();
        copy.staged_identity = Some(fs.reserve_project_copy(&copy).unwrap());
        fs.stage_project_copy(&copy, &payload).unwrap();
        copy.content_hash = Some(fs.project_copy_hash(&copy.staging_path).unwrap());
        fs.write_enable_journal(
            &home.library_root,
            &EnableJournal {
                project_links: vec![],
                project_copy: Some(copy.clone()),
                version: 1,
                operation_id: "enable-copy-interrupted".into(),
                phase,
                cells: vec![],
            },
        )
        .unwrap();
        fs.publish_project_copy(&copy).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "# New version must never be recovered",
        )
        .unwrap();
        for _ in 0..2 {
            let health = HealthApi::new(
                MaintenanceService::for_tests(
                    home.runtime.clone(),
                    home.filesystem.clone(),
                    home.write_gate.clone(),
                )
                .with_library_root(home.library_root.clone())
                .begin_startup(),
            );
            health.run_activation_health_check().unwrap();
        }
        if phase == Phase::Committed {
            assert_eq!(
                std::fs::read_to_string(copy.entry_path.join("SKILL.md")).unwrap(),
                "# Frozen delivered version\n"
            );
        } else {
            assert!(!copy.entry_path.exists());
        }
        assert!(!copy.staging_path.exists());
    }
}

#[test]
fn undo_preserves_permission_only_edits() {
    use skill_man_lib::tauri_adapter::dto::EnableOperationRequestDto;
    use std::os::unix::fs::PermissionsExt;
    for root in [true, false] {
        let (_home, api) = harness();
        let project = tempfile::tempdir().unwrap();
        let p = plan(&api, &project);
        let result = api
            .apply_project_enable(ApplyGlobalEnableRequestDto {
                plan_token: p.plan_token,
            })
            .unwrap();
        let copy = project.path().join(".agents/skills/skill-authoring");
        let changed = if root {
            copy.clone()
        } else {
            copy.join("SKILL.md")
        };
        std::fs::set_permissions(changed, std::fs::Permissions::from_mode(0o700)).unwrap();
        let undo = api
            .undo_project_enable(EnableOperationRequestDto {
                operation_id: result.operation_id,
            })
            .unwrap();
        assert!(!undo.cells[0].undone);
        assert!(copy.join("SKILL.md").exists());
    }
}
#[test]
fn indirect_existing_directory_cycles_are_not_reused() {
    use std::os::unix::fs::symlink;
    let (_home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    let copy = project.path().join(".agents/skills/skill-authoring");
    std::fs::create_dir_all(copy.join("x")).unwrap();
    std::fs::create_dir(copy.join("y")).unwrap();
    std::fs::write(copy.join("SKILL.md"), "# Project").unwrap();
    for (from, to) in [
        ("alias_x", "x"),
        ("alias_y", "y"),
        ("x/to_y", "../alias_y"),
        ("y/to_x", "../alias_x"),
    ] {
        symlink(to, copy.join(from)).unwrap();
    }
    assert_eq!(
        serde_json::to_value(plan(&api, &project)).unwrap()["cells"][0]["eligibility"],
        "blocked"
    );
}

#[test]
fn reserved_partial_stage_is_cleaned_by_formal_startup() {
    use skill_man_lib::core::maintenance::MaintenanceService;
    use skill_man_lib::seams::filesystem::{
        ActivationReplacePhase as Phase, EnableJournal, ProjectCopyJournal,
    };
    use skill_man_lib::tauri_adapter::health_api::HealthApi;
    use std::path::Path;
    let (home, _) = harness();
    let project = tempfile::tempdir().unwrap();
    let fs = &home.filesystem;
    let root = fs.directory_fingerprint(project.path()).unwrap();
    let resolution = fs
        .resolve_project_target(&root.canonical_path, Path::new(".agents/skills"))
        .unwrap();
    let parent = fs.prepare_project_copy_parent(&root, &resolution).unwrap();
    let mut copy = ProjectCopyJournal {
        cleanup_authorized: false,
        target_resolution: fs
            .resolve_project_target(&root.canonical_path, Path::new(".agents/skills"))
            .unwrap(),
        project_root: root,
        parent: parent.clone(),
        entry_path: parent.canonical_path.join("skill-authoring"),
        staging_path: parent.canonical_path.join(".skillman-partial"),
        staged_identity: None,
        content_hash: None,
        phase: Phase::Applying,
    };
    copy.staged_identity = Some(fs.reserve_project_copy(&copy).unwrap());
    fs.write_enable_journal(
        &home.library_root,
        &EnableJournal {
            project_links: vec![],
            project_copy: Some(copy.clone()),
            version: 1,
            operation_id: "enable-partial-copy".into(),
            phase: Phase::Applying,
            cells: vec![],
        },
    )
    .unwrap();
    std::fs::write(copy.staging_path.join("incomplete"), "partial").unwrap();
    let health = HealthApi::new(
        MaintenanceService::for_tests(
            home.runtime.clone(),
            home.filesystem.clone(),
            home.write_gate.clone(),
        )
        .with_library_root(home.library_root.clone())
        .begin_startup(),
    );
    health.run_activation_health_check().unwrap();
    assert!(!copy.staging_path.exists());
    assert!(!copy.entry_path.exists());
}
#[test]
fn restarting_after_project_edits_and_move_finalizes_the_delivered_copy() {
    use skill_man_lib::core::maintenance::MaintenanceService;
    use skill_man_lib::tauri_adapter::health_api::HealthApi;
    let (home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    let p = plan(&api, &project);
    api.apply_project_enable(ApplyGlobalEnableRequestDto {
        plan_token: p.plan_token,
    })
    .unwrap();
    std::fs::write(
        project
            .path()
            .join(".agents/skills/skill-authoring/SKILL.md"),
        "# Project edits",
    )
    .unwrap();
    let moved = project.path().with_extension("moved");
    std::fs::rename(project.path(), &moved).unwrap();
    drop(api);
    let health = HealthApi::new(
        MaintenanceService::for_tests(
            home.runtime.clone(),
            home.filesystem.clone(),
            home.write_gate.clone(),
        )
        .with_library_root(home.library_root.clone())
        .begin_startup(),
    );
    health.run_activation_health_check().unwrap();
    assert_eq!(
        std::fs::read_to_string(moved.join(".agents/skills/skill-authoring/SKILL.md")).unwrap(),
        "# Project edits"
    );
    std::fs::remove_dir_all(moved).unwrap();
}
#[test]
fn safe_relative_project_aliases_reuse_the_existing_physical_spelling() {
    use std::os::unix::fs::symlink;
    let (_home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("shared/skills/SKILL-AUTHORING")).unwrap();
    std::fs::write(
        project
            .path()
            .join("shared/skills/SKILL-AUTHORING/SKILL.md"),
        "# Project",
    )
    .unwrap();
    symlink("shared", project.path().join(".agents")).unwrap();
    let p = plan(&api, &project);
    assert!(
        p.cells[0]
            .entry_path
            .ends_with("shared/skills/SKILL-AUTHORING")
    );
    assert_eq!(
        serde_json::to_value(&p).unwrap()["cells"][0]["eligibility"],
        "no_op"
    );
    api.apply_project_enable(ApplyGlobalEnableRequestDto {
        plan_token: p.plan_token,
    })
    .unwrap();
}
#[test]
fn undo_removes_only_the_unchanged_new_copy_and_scope_never_falls_back_to_direct_links() {
    use skill_man_lib::tauri_adapter::dto::EnableOperationRequestDto;
    let (_home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    let p = plan(&api, &project);
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: p.plan_token,
        })
        .unwrap();
    let undo = api
        .undo_project_enable(EnableOperationRequestDto {
            operation_id: result.operation_id,
        })
        .unwrap();
    assert!(undo.cells[0].undone);
    assert!(
        !project
            .path()
            .join(".agents/skills/skill-authoring")
            .exists()
    );
    for (skills, agents) in [
        (vec!["skill-authoring".into()], vec!["claude-code".into()]),
        (vec!["skill-authoring".into(), "media-xray".into()], vec![]),
    ] {
        assert!(
            api.plan_project_enable(PlanProjectEnableRequestDto {
                skill_ids: skills,
                agent_ids: agents,
                project_folder: project.path().to_string_lossy().into_owned(),
                cell_resolutions: vec![]
            })
            .is_err()
        );
    }
    assert!(!project.path().join(".claude").exists());
}

#[test]
fn undo_safely_removes_readonly_payload_directories() {
    use skill_man_lib::tauri_adapter::dto::EnableOperationRequestDto;
    use std::os::unix::fs::PermissionsExt;
    let (home, api) = harness();
    let project = tempfile::tempdir().unwrap();
    let source = home.path().join("Projects/skill-authoring/readonly");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("file"), "payload").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o555)).unwrap();
    let p = plan(&api, &project);
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: p.plan_token,
        })
        .unwrap();
    assert_eq!(result.cells[0].copy_ready, Some(true));
    let undo = api
        .undo_project_enable(EnableOperationRequestDto {
            operation_id: result.operation_id,
        })
        .unwrap();
    assert!(undo.cells[0].undone);
    assert!(
        !project
            .path()
            .join(".agents/skills/skill-authoring")
            .exists()
    );
    std::fs::set_permissions(source, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn startup_distinguishes_unvalidated_quarantine_from_authorized_partial_cleanup() {
    use skill_man_lib::core::maintenance::MaintenanceService;
    use skill_man_lib::seams::filesystem::{
        ActivationReplacePhase as Phase, EnableJournal, ProjectCopyJournal,
    };
    use skill_man_lib::tauri_adapter::health_api::HealthApi;
    use std::path::Path;
    for cleanup_authorized in [false, true] {
        let (home, _) = harness();
        let project = tempfile::tempdir().unwrap();
        let fs = &home.filesystem;
        let root = fs.directory_fingerprint(project.path()).unwrap();
        let resolution = fs
            .resolve_project_target(&root.canonical_path, Path::new(".agents/skills"))
            .unwrap();
        let parent = fs.prepare_project_copy_parent(&root, &resolution).unwrap();
        let mut copy = ProjectCopyJournal {
            cleanup_authorized,
            target_resolution: fs
                .resolve_project_target(&root.canonical_path, Path::new(".agents/skills"))
                .unwrap(),
            project_root: root,
            parent: parent.clone(),
            entry_path: parent.canonical_path.join("skill-authoring"),
            staging_path: parent.canonical_path.join(".skillman-quarantine"),
            staged_identity: None,
            content_hash: None,
            phase: Phase::Undoing,
        };
        let payload = fs
            .project_copy_payload(&home.path().join("Projects/skill-authoring"), false)
            .unwrap();
        copy.staged_identity = Some(fs.reserve_project_copy(&copy).unwrap());
        fs.stage_project_copy(&copy, &payload).unwrap();
        copy.content_hash = Some(fs.project_copy_hash(&copy.staging_path).unwrap());
        fs.publish_project_copy(&copy).unwrap();
        std::fs::rename(&copy.entry_path, &copy.staging_path).unwrap();
        if cleanup_authorized {
            std::fs::remove_file(copy.staging_path.join("SKILL.md")).unwrap();
        } else {
            std::fs::write(
                copy.staging_path.join("SKILL.md"),
                "# Concurrent project edit",
            )
            .unwrap();
        }
        fs.write_enable_journal(
            &home.library_root,
            &EnableJournal {
                project_links: vec![],
                version: 1,
                operation_id: "enable-quarantine-crash".into(),
                project_copy: Some(copy.clone()),
                phase: Phase::Undoing,
                cells: vec![],
            },
        )
        .unwrap();
        for _ in 0..2 {
            let health = HealthApi::new(
                MaintenanceService::for_tests(
                    home.runtime.clone(),
                    home.filesystem.clone(),
                    home.write_gate.clone(),
                )
                .with_library_root(home.library_root.clone())
                .begin_startup(),
            );
            let recovered = health.run_activation_health_check();
            if cleanup_authorized {
                recovered.unwrap();
                assert!(!copy.staging_path.exists());
            } else {
                assert!(recovered.is_err());
                assert_eq!(
                    std::fs::read_to_string(copy.staging_path.join("SKILL.md")).unwrap(),
                    "# Concurrent project edit"
                );
            }
        }
        assert!(!copy.entry_path.exists());
    }
}

#[test]
fn multiple_agents_share_a_portable_project_copy_and_undo_links_first() {
    let (home, api) = harness();
    configure_project_agent(&home, "claude-code", ".claude/skills");
    configure_project_agent(&home, "codex", ".codex/skills");
    let project = tempfile::tempdir().unwrap();
    let preview = api
        .plan_project_enable(PlanProjectEnableRequestDto {
            skill_ids: vec!["skill-authoring".into()],
            project_folder: project.path().to_string_lossy().into_owned(),
            agent_ids: vec!["claude-code".into(), "codex".into()],
            cell_resolutions: vec![],
        })
        .unwrap();
    assert!(!project.path().join(".agents").exists());
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: preview.plan_token,
        })
        .unwrap();
    assert!(
        result.cells.iter().all(
            |c| serde_json::to_value(c).unwrap()["outcome"] == "succeeded"
                || serde_json::to_value(c).unwrap()["outcome"] == "no_op"
        )
    );
    let copy = project.path().join(".agents/skills/skill-authoring");
    let link = project.path().join(".claude/skills/skill-authoring");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        std::path::Path::new("../../.agents/skills/skill-authoring")
    );
    assert_eq!(
        std::fs::read(link.join("SKILL.md")).unwrap(),
        std::fs::read(copy.join("SKILL.md")).unwrap()
    );
    let undone = api
        .undo_project_enable(
            skill_man_lib::tauri_adapter::dto::EnableOperationRequestDto {
                operation_id: result.operation_id,
            },
        )
        .unwrap();
    assert!(undone.cells.iter().all(|c| c.undone));
    assert!(!copy.exists());
    assert!(std::fs::symlink_metadata(link).is_err());
}

fn configure_project_agent(home: &BoundTestHome, id: &str, directory: &str) {
    use skill_man_lib::core::agent_configuration::{
        AgentConfigurationDraft, AgentConfigurationService, AgentRootDraft, PresetRegistry,
    };
    let service = AgentConfigurationService::new(
        home.runtime.clone(),
        Arc::new(MacOsAgentConfigurationFileSystem::new(home.path().into())),
        home.write_gate.clone(),
        PresetRegistry::system(),
    );
    let config = service
        .snapshot()
        .unwrap()
        .configurations
        .into_iter()
        .find(|a| a.agent_id == id)
        .unwrap();
    let plan = service
        .plan_edit(
            id,
            AgentConfigurationDraft {
                preset_key: config.preset_key,
                name: config.name,
                project_skills_dir: Some(directory.into()),
                roots: config
                    .roots
                    .into_iter()
                    .map(|r| AgentRootDraft {
                        configured_path: r.configured_path,
                        role: r.role,
                    })
                    .collect(),
            },
        )
        .unwrap();
    service.apply(&plan.plan_token).unwrap();
}

fn plan_agents(
    api: &EnableApi,
    project: &tempfile::TempDir,
    ids: &[&str],
) -> skill_man_lib::tauri_adapter::dto::EnablePlanDto {
    api.plan_project_enable(PlanProjectEnableRequestDto {
        skill_ids: vec!["skill-authoring".into()],
        project_folder: project.path().to_string_lossy().into_owned(),
        agent_ids: ids.iter().map(|s| (*s).into()).collect(),
        cell_resolutions: vec![],
    })
    .unwrap()
}

#[test]
fn reused_copy_shared_aliases_and_correct_links_are_no_ops_after_project_move() {
    let (home, api) = harness();
    configure_project_agent(&home, "claude-code", ".claude/skills");
    configure_project_agent(&home, "codex", ".codex/skills");
    let project = tempfile::tempdir().unwrap();
    let copy = project.path().join(".agents/skills/skill-authoring");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join("SKILL.md"), "# Project edition").unwrap();
    std::fs::create_dir(project.path().join(".claude")).unwrap();
    std::fs::create_dir(project.path().join("shared")).unwrap();
    std::os::unix::fs::symlink("../shared", project.path().join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink("shared", project.path().join(".codex")).unwrap();
    // Both configured aliases resolve to one physical skills container.
    configure_project_agent(&home, "codex", ".codex");
    let preview = plan_agents(&api, &project, &["claude-code"]);
    assert_eq!(preview.cells.len(), 2);
    assert_eq!(preview.cells[1].affected_agent_ids.len(), 2);
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: preview.plan_token,
        })
        .unwrap();
    assert_eq!(api.list_recent_project_folders().unwrap().len(), 1);
    let link = project.path().join("shared/skill-authoring");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        std::path::Path::new("../.agents/skills/skill-authoring")
    );
    let second = plan_agents(&api, &project, &["claude-code", "codex"]);
    assert!(
        second
            .cells
            .iter()
            .all(|c| serde_json::to_value(c).unwrap()["eligibility"] == "no_op")
    );
    api.finalize_project_enable(
        skill_man_lib::tauri_adapter::dto::EnableOperationRequestDto {
            operation_id: result.operation_id,
        },
    )
    .unwrap();
    let moved = tempfile::tempdir().unwrap();
    std::fs::rename(project.path(), moved.path().join("project")).unwrap();
    std::fs::remove_dir_all(home.path().join("Projects/skill-authoring")).unwrap();
    for entry in [".claude/skills", ".codex"] {
        assert_eq!(
            std::fs::read_to_string(
                moved
                    .path()
                    .join("project")
                    .join(entry)
                    .join("skill-authoring/SKILL.md")
            )
            .unwrap(),
            "# Project edition"
        );
    }
}

#[test]
fn conflicts_are_preserved_and_changed_dependency_retains_copy_during_partial_undo() {
    let (home, api) = harness();
    configure_project_agent(&home, "claude-code", ".claude/skills");
    configure_project_agent(&home, "codex", ".codex/skills");
    let project = tempfile::tempdir().unwrap();
    let preview = plan_agents(&api, &project, &["claude-code", "codex"]);
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: preview.plan_token,
        })
        .unwrap();
    let changed = project.path().join(".claude/skills/skill-authoring");
    std::fs::remove_file(&changed).unwrap();
    std::fs::write(&changed, "external content").unwrap();
    let undone = api
        .undo_project_enable(
            skill_man_lib::tauri_adapter::dto::EnableOperationRequestDto {
                operation_id: result.operation_id,
            },
        )
        .unwrap();
    assert!(undone.cells.iter().any(|c| c.undone));
    assert_eq!(undone.cells.iter().filter(|c| !c.undone).count(), 2);
    assert!(
        project
            .path()
            .join(".agents/skills/skill-authoring/SKILL.md")
            .is_file()
    );
    assert_eq!(
        std::fs::read_to_string(changed).unwrap(),
        "external content"
    );
    assert!(
        std::fs::symlink_metadata(project.path().join(".codex/skills/skill-authoring")).is_err()
    );
    let retry = plan_agents(&api, &project, &["claude-code", "codex"]);
    assert!(
        retry
            .cells
            .iter()
            .any(|c| serde_json::to_value(c).unwrap()["eligibility"] == "conflict")
    );
    let retry = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: retry.plan_token,
        })
        .unwrap();
    assert!(
        retry
            .cells
            .iter()
            .any(|c| serde_json::to_value(c).unwrap()["outcome"] == "skipped")
    );
    assert!(
        retry
            .cells
            .iter()
            .any(|c| serde_json::to_value(c).unwrap()["outcome"] == "succeeded")
    );
}

#[test]
fn unsafe_aliases_and_overlapping_artifacts_are_blocked_without_touching_occupants() {
    for alias in ["absolute", "outside", "cycle", "overlap", "base"] {
        let (home, api) = harness();
        let project = tempfile::tempdir().unwrap();
        configure_project_agent(
            &home,
            "claude-code",
            if alias == "overlap" {
                ".agents/skills/skill-authoring/nested"
            } else {
                ".claude/skills"
            },
        );
        std::fs::create_dir(project.path().join(".claude")).unwrap();
        match alias {
            "absolute" => {
                std::fs::create_dir(project.path().join("inner")).unwrap();
                std::os::unix::fs::symlink(
                    project.path().join("inner"),
                    project.path().join(".claude/skills"),
                )
                .unwrap();
            }
            "outside" => {
                std::os::unix::fs::symlink("../../outside", project.path().join(".claude/skills"))
                    .unwrap();
            }
            "cycle" => {
                std::os::unix::fs::symlink("skills", project.path().join(".claude/skills"))
                    .unwrap();
            }
            "base" => {
                std::fs::create_dir_all(project.path().join(".agents/skills")).unwrap();
                std::os::unix::fs::symlink(
                    "../.agents/skills",
                    project.path().join(".claude/skills"),
                )
                .unwrap();
            }
            _ => {}
        }
        let preview = plan_agents(&api, &project, &["claude-code"]);
        if alias == "base" {
            assert_eq!(preview.cells.len(), 1);
            assert_eq!(preview.cells[0].affected_agent_ids, vec!["claude-code"]);
        } else {
            assert_eq!(
                serde_json::to_value(&preview.cells[1]).unwrap()["eligibility"],
                "blocked"
            );
        }
        let result = api
            .apply_project_enable(ApplyGlobalEnableRequestDto {
                plan_token: preview.plan_token,
            })
            .unwrap();
        assert_eq!(
            serde_json::to_value(&result.cells[0]).unwrap()["outcome"],
            "succeeded"
        );
        assert!(
            !project
                .path()
                .join(".agents/skills/skill-authoring/nested")
                .exists()
        );
    }
}

#[test]
fn a_runtime_agent_failure_keeps_copy_and_successful_sibling_and_retry_repreviews() {
    use std::os::unix::fs::PermissionsExt;
    let (home, api) = harness();
    configure_project_agent(&home, "claude-code", ".claude/skills");
    configure_project_agent(&home, "codex", ".codex/skills");
    let project = tempfile::tempdir().unwrap();
    let readonly = project.path().join(".claude/skills");
    std::fs::create_dir_all(&readonly).unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555)).unwrap();
    let preview = plan_agents(&api, &project, &["claude-code", "codex"]);
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: preview.plan_token,
        })
        .unwrap();
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        result
            .cells
            .iter()
            .filter(|c| serde_json::to_value(c).unwrap()["outcome"] == "succeeded")
            .count(),
        2
    );
    assert_eq!(
        result
            .cells
            .iter()
            .filter(|c| serde_json::to_value(c).unwrap()["outcome"] == "failed")
            .count(),
        1
    );
    let preview = plan_agents(&api, &project, &["claude-code", "codex"]);
    let retried = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: preview.plan_token,
        })
        .unwrap();
    assert!(retried.cells.iter().all(|c| matches!(
        serde_json::to_value(c).unwrap()["outcome"].as_str(),
        Some("succeeded" | "no_op")
    )));
}

#[test]
fn unavailable_copy_marks_every_dependency_not_attempted() {
    use std::os::unix::fs::PermissionsExt;
    let (home, api) = harness();
    configure_project_agent(&home, "claude-code", ".claude/skills");
    let project = tempfile::tempdir().unwrap();
    let base = project.path().join(".agents");
    std::fs::create_dir(&base).unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o555)).unwrap();
    let preview = plan_agents(&api, &project, &["claude-code"]);
    let result = api
        .apply_project_enable(ApplyGlobalEnableRequestDto {
            plan_token: preview.plan_token,
        })
        .unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        serde_json::to_value(&result.cells[0]).unwrap()["outcome"],
        "failed"
    );
    assert_eq!(
        serde_json::to_value(&result.cells[1]).unwrap()["outcome"],
        "not_attempted"
    );
    assert!(!project.path().join(".claude").exists());
}

#[test]
fn formal_startup_preserves_committed_siblings_and_resumes_dependency_undo() {
    use skill_man_lib::core::maintenance::MaintenanceService;
    use skill_man_lib::seams::filesystem::{ActivationReplacePhase as Phase, EnableJournal};
    use skill_man_lib::tauri_adapter::health_api::HealthApi;
    for scenario in [
        "committed",
        "before_second_link",
        "uncaptured_link",
        "undo",
        "undo_one_removed",
        "undo_changed",
    ] {
        let (home, api) = harness();
        configure_project_agent(&home, "claude-code", ".claude/skills");
        configure_project_agent(&home, "codex", ".codex/skills");
        let project = tempfile::tempdir().unwrap();
        let preview = plan_agents(&api, &project, &["claude-code", "codex"]);
        let result = api
            .apply_project_enable(ApplyGlobalEnableRequestDto {
                plan_token: preview.plan_token,
            })
            .unwrap();
        // Inject the durable state at a process interruption, then enter through formal startup.
        let path = home
            .library_root
            .join("operations")
            .join(&result.operation_id)
            .join("enable-journal.json");
        let mut journal: EnableJournal =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if scenario == "before_second_link" {
            std::fs::remove_file(&journal.project_links[1].entry_path).unwrap();
            journal.project_links[1].phase = Phase::Applying;
            journal.project_links[1].occupant = None;
        } else if scenario == "uncaptured_link" {
            journal.project_links[1].phase = Phase::Applying;
            journal.project_links[1].occupant = None;
        } else if scenario.starts_with("undo") {
            journal.phase = Phase::Undoing;
            for link in &mut journal.project_links {
                link.phase = Phase::Undoing;
            }
            if scenario == "undo_one_removed" {
                std::fs::remove_file(&journal.project_links[0].entry_path).unwrap();
            }
            if scenario == "undo_changed" {
                std::fs::remove_file(&journal.project_links[0].entry_path).unwrap();
                std::fs::write(&journal.project_links[0].entry_path, "external").unwrap();
            }
        }
        home.filesystem
            .write_enable_journal(&home.library_root, &journal)
            .unwrap();
        std::fs::remove_dir_all(home.path().join("Projects/skill-authoring")).unwrap();
        for _ in 0..2 {
            let health = HealthApi::new(
                MaintenanceService::for_tests(
                    home.runtime.clone(),
                    home.filesystem.clone(),
                    home.write_gate.clone(),
                )
                .with_library_root(home.library_root.clone())
                .begin_startup(),
            );
            let recovered = health.run_activation_health_check();
            if scenario == "uncaptured_link" {
                assert!(recovered.is_err());
            } else {
                recovered.unwrap();
            }
        }
        if scenario == "uncaptured_link" {
            assert!(
                journal
                    .project_links
                    .iter()
                    .all(|link| link.entry_path.join("SKILL.md").is_file())
            );
        }
        let copy = project.path().join(".agents/skills/skill-authoring");
        assert_eq!(
            copy.exists(),
            !matches!(scenario, "undo" | "undo_one_removed")
        );
        if scenario == "committed" || scenario == "before_second_link" {
            assert!(
                journal.project_links[0]
                    .entry_path
                    .join("SKILL.md")
                    .is_file()
            );
        }
        if scenario.starts_with("undo") {
            assert!(std::fs::symlink_metadata(&journal.project_links[1].entry_path).is_err());
        }
        if scenario == "undo_changed" {
            assert_eq!(
                std::fs::read_to_string(&journal.project_links[0].entry_path).unwrap(),
                "external"
            );
        }
    }
}
