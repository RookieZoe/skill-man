use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, params};
use skill_man_lib::adapters::agent_configuration_fs::MacOsAgentConfigurationFileSystem;
use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::core::agent_configuration::{
    AgentConfigurationDraft, AgentConfigurationError, AgentConfigurationService, AgentRootDraft,
    AgentRootRole, PresetRegistry,
};
use skill_man_lib::core::home::BoundHome;
use skill_man_lib::core::write_gate::{WriteGate, WriteGateState};
use skill_man_lib::seams::agent_configuration_store::{
    AgentConfigurationStore, RecentProjectFolder,
};
use skill_man_lib::seams::catalog_probe::CatalogProbe;

fn open_service(
    home: &std::path::Path,
) -> (
    AgentConfigurationService,
    Arc<SqliteCatalogStore>,
    Arc<WriteGate>,
) {
    let catalog_path = home.join("skill-man.sqlite3");
    let sqlite = Arc::new(SqliteCatalogStore::open(&catalog_path).expect("open catalog"));
    let store: Arc<dyn AgentConfigurationStore> = sqlite.clone();
    let gate = Arc::new(WriteGate::new(WriteGateState::Open(BoundHome::test_value(
        "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
        home.to_path_buf(),
    ))));
    let filesystem = Arc::new(MacOsAgentConfigurationFileSystem::new(
        home.parent().expect("home parent").to_path_buf(),
    ));
    let service =
        AgentConfigurationService::new(store, filesystem, gate.clone(), PresetRegistry::system());
    (service, sqlite, gate)
}

#[test]
fn only_general_preset_owns_the_shared_skill_directories() {
    let registry = PresetRegistry::system();
    let general = registry.get("general").unwrap();
    assert_eq!(general.roots, vec![PathBuf::from("~/.agents/skills")]);
    assert_eq!(general.activation_target, PathBuf::from("~/.agents/skills"));
    assert_eq!(general.project_skills_dir, PathBuf::from(".agents/skills"));
    for preset in registry
        .presets()
        .iter()
        .filter(|p| p.preset_key != "general")
    {
        assert!(!preset.roots.contains(&PathBuf::from("~/.agents/skills")));
        assert_ne!(preset.project_skills_dir, PathBuf::from(".agents/skills"));
        assert!(!preset.roots.is_empty());
    }
    let codex = registry.get("codex").unwrap();
    assert_eq!(codex.roots, vec![codex.activation_target.clone()]);
    assert_eq!(codex.project_skills_dir, PathBuf::from(".codex/skills"));
    assert!(
        registry.get("zed").is_some(),
        "existing configurations stay editable"
    );
}

#[test]
fn create_configuration_persists_multiple_roots_one_target_and_nfkc_name_identity() {
    let temp = tempfile::tempdir().expect("temp directory");
    let home = temp.path().join("bound-home");
    let scan_root = temp.path().join("scan-root");
    let target_root = temp.path().join("target-root");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&scan_root).expect("create scan root");
    std::fs::create_dir_all(&target_root).expect("create target root");
    let (service, _sqlite, _gate) = open_service(&home);

    let fresh = service.snapshot().expect("fresh configuration snapshot");
    assert!(fresh.configurations.is_empty());
    assert_eq!(fresh.presets.len(), 9);

    let plan = service
        .plan_create(AgentConfigurationDraft {
            preset_key: None,
            name: "Writer".into(),
            roots: vec![
                AgentRootDraft {
                    configured_path: scan_root.clone(),
                    role: AgentRootRole::ScanOnly,
                },
                AgentRootDraft {
                    configured_path: target_root.clone(),
                    role: AgentRootRole::ActivationTarget,
                },
            ],
            project_skills_dir: Some(PathBuf::from(".writer/skills")),
        })
        .expect("plan configuration");
    let applied = service
        .apply(&plan.plan_token)
        .expect("apply configuration");

    let snapshot = service.snapshot().expect("configuration snapshot");
    assert_eq!(snapshot.configurations.len(), 1);
    let configuration = &snapshot.configurations[0];
    assert_eq!(configuration.agent_id, applied.agent_id);
    assert_eq!(configuration.roots.len(), 2);
    assert_eq!(
        configuration
            .roots
            .iter()
            .filter(|root| root.role == AgentRootRole::ActivationTarget)
            .count(),
        1
    );
    assert_eq!(
        configuration.project_skills_dir,
        Some(PathBuf::from(".writer/skills"))
    );

    let duplicate = service.plan_create(AgentConfigurationDraft {
        preset_key: None,
        name: "ＷＲＩＴＥＲ".into(),
        roots: vec![AgentRootDraft {
            configured_path: temp.path().join("other-target"),
            role: AgentRootRole::ActivationTarget,
        }],
        project_skills_dir: None,
    });
    assert!(matches!(
        duplicate,
        Err(AgentConfigurationError::NameConflict { .. })
    ));
}

#[test]
fn apply_revalidates_target_occupancy_and_write_gate_generation() {
    let temp = tempfile::tempdir().expect("temp directory");
    let home = temp.path().join("bound-home");
    let target = temp.path().join("target");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&target).expect("create target");
    let (service, _sqlite, gate) = open_service(&home);
    let draft = || AgentConfigurationDraft {
        preset_key: None,
        name: "Planner".into(),
        roots: vec![AgentRootDraft {
            configured_path: target.clone(),
            role: AgentRootRole::ActivationTarget,
        }],
        project_skills_dir: None,
    };

    let occupancy_plan = service.plan_create(draft()).expect("plan target");
    std::fs::write(target.join("appeared-after-plan"), "occupied").expect("change occupancy");
    assert!(matches!(
        service.apply(&occupancy_plan.plan_token),
        Err(AgentConfigurationError::PlanStale)
    ));
    std::fs::remove_file(target.join("appeared-after-plan")).expect("restore occupancy");

    let gate_plan = service.plan_create(draft()).expect("replan target");
    let bound = gate.bound_home().expect("bound home");
    gate.transition_to(WriteGateState::Open(bound))
        .expect("bump gate generation");
    assert!(matches!(
        service.apply(&gate_plan.plan_token),
        Err(AgentConfigurationError::PlanStale)
    ));

    let current_plan = service.plan_create(draft()).expect("current plan");
    service
        .apply(&current_plan.plan_token)
        .expect("apply current plan");
}

#[test]
fn explicit_configuration_plan_creates_only_the_missing_target() {
    let temp = tempfile::tempdir().expect("temp directory");
    let home = temp.path().join("bound-home");
    let target = temp.path().join("missing").join("target");
    let missing_scan_root = temp.path().join("missing-scan-root");
    std::fs::create_dir_all(&home).expect("create home");
    let (service, _sqlite, _gate) = open_service(&home);

    let plan = service
        .plan_create(AgentConfigurationDraft {
            preset_key: None,
            name: "Missing roots".into(),
            roots: vec![
                AgentRootDraft {
                    configured_path: missing_scan_root.clone(),
                    role: AgentRootRole::ScanOnly,
                },
                AgentRootDraft {
                    configured_path: target.clone(),
                    role: AgentRootRole::ActivationTarget,
                },
            ],
            project_skills_dir: None,
        })
        .expect("plan missing Target");
    assert!(plan.target_will_be_created);
    assert!(!target.exists());
    assert!(!missing_scan_root.exists());

    service
        .apply(&plan.plan_token)
        .expect("apply missing Target");
    assert!(target.is_dir());
    assert!(
        !missing_scan_root.exists(),
        "scan-only roots are never created as a configuration side effect"
    );
}

#[test]
fn plan_rejects_home_overlap_and_unsafe_project_directory() {
    let temp = tempfile::tempdir().expect("temp directory");
    let home = temp.path().join("bound-home");
    let target = temp.path().join("target");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&target).expect("create target");
    let (service, _sqlite, _gate) = open_service(&home);

    let overlap = service.plan_create(AgentConfigurationDraft {
        preset_key: None,
        name: "Overlap".into(),
        roots: vec![AgentRootDraft {
            configured_path: home.join("nested"),
            role: AgentRootRole::ActivationTarget,
        }],
        project_skills_dir: None,
    });
    assert!(matches!(
        overlap,
        Err(AgentConfigurationError::HomeOverlap { .. })
    ));

    let unsafe_project = service.plan_create(AgentConfigurationDraft {
        preset_key: None,
        name: "Unsafe project".into(),
        roots: vec![AgentRootDraft {
            configured_path: target,
            role: AgentRootRole::ActivationTarget,
        }],
        project_skills_dir: Some(PathBuf::from("../outside")),
    });
    assert!(matches!(
        unsafe_project,
        Err(AgentConfigurationError::InvalidProjectSkillsDir)
    ));
}

#[test]
fn shared_target_detach_preserves_activations_and_last_reference_is_blocked() {
    let temp = tempfile::tempdir().expect("temp directory");
    let home = temp.path().join("bound-home");
    let shared_target = temp.path().join("shared-target");
    let replacement_target = temp.path().join("replacement-target");
    let entity = temp.path().join("managed-entity");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&shared_target).expect("create shared target");
    std::fs::create_dir_all(&replacement_target).expect("create replacement target");
    std::fs::create_dir_all(&entity).expect("create entity");
    let catalog_path = home.join("skill-man.sqlite3");
    let (service, _sqlite, _gate) = open_service(&home);

    let draft = |name: &str, target: &std::path::Path| AgentConfigurationDraft {
        preset_key: None,
        name: name.into(),
        roots: vec![AgentRootDraft {
            configured_path: target.to_path_buf(),
            role: AgentRootRole::ActivationTarget,
        }],
        project_skills_dir: None,
    };
    let first = service
        .plan_create(draft("First", &shared_target))
        .and_then(|plan| service.apply(&plan.plan_token))
        .expect("create first consumer");
    let second = service
        .plan_create(draft("Second", &shared_target))
        .and_then(|plan| service.apply(&plan.plan_token))
        .expect("create second consumer");
    let snapshot = service.snapshot().expect("shared snapshot");
    let root_id = snapshot.configurations[0].roots[0].root_id.clone();
    assert_eq!(
        snapshot.configurations[1].roots[0].root_id, root_id,
        "same canonical Root is shared"
    );

    let connection = Connection::open(&catalog_path).expect("open catalog");
    connection
        .execute(
            "INSERT INTO skills (
                id, directory_name, directory_identity_key, display_name, description,
                source_kind, library_entry_path, final_entity_path,
                recorded_content_hash, health, created_at, updated_at
             ) VALUES (
                'skill-1', 'review', 'review', 'Review', '', 'link', NULL, ?1,
                NULL, 'healthy', '1', '1'
             )",
            [entity.to_string_lossy().as_ref()],
        )
        .expect("seed skill");
    connection
        .execute(
            "INSERT INTO activations (
                skill_id, target_root_id, directory_identity_key, desired_enabled,
                expected_entry_path, expected_target_path, observed_state,
                last_enabled_at, last_checked_at
             ) VALUES ('skill-1', ?1, 'review', 1, ?2, ?3, 'present', '2', '3')",
            params![
                root_id,
                shared_target.join("review").to_string_lossy(),
                entity.to_string_lossy()
            ],
        )
        .expect("seed Target-scoped Activation");
    drop(connection);

    let detach = service
        .plan_delete(&first.agent_id)
        .expect("plan non-last detach");
    assert!(detach.blocking_activation_skill_ids.is_empty());
    assert_eq!(detach.retained_activation_count, 1);
    service
        .apply(&detach.plan_token)
        .expect("detach first consumer");

    let blocked_edit = service
        .plan_edit(&second.agent_id, draft("Second", &replacement_target))
        .expect("plan last-reference edit");
    assert_eq!(blocked_edit.blocking_activation_skill_ids, vec!["skill-1"]);
    assert!(matches!(
        service.apply(&blocked_edit.plan_token),
        Err(AgentConfigurationError::TargetInUse { .. })
    ));

    let third = service
        .plan_create(draft("Third", &shared_target))
        .and_then(|plan| service.apply(&plan.plan_token))
        .expect("create replacement consumer");
    let safe_edit = service
        .plan_edit(&second.agent_id, draft("Second", &replacement_target))
        .expect("plan shared Target detach");
    assert!(safe_edit.blocking_activation_skill_ids.is_empty());
    assert_eq!(safe_edit.retained_activation_count, 1);
    service
        .apply(&safe_edit.plan_token)
        .expect("move Agent relationship only");

    let connection = Connection::open(&catalog_path).expect("inspect target state");
    let activation_root: String = connection
        .query_row(
            "SELECT target_root_id FROM activations WHERE skill_id = 'skill-1'",
            [],
            |row| row.get(0),
        )
        .expect("activation root");
    assert_eq!(activation_root, root_id, "Activation is never migrated");
    drop(connection);

    let last_delete = service
        .plan_delete(&third.agent_id)
        .expect("plan last shared consumer delete");
    assert_eq!(last_delete.blocking_activation_skill_ids, vec!["skill-1"]);
    // Disable keeps its historical row, but a disabled entry must no longer
    // block removing the last Agent Configuration that consumed this Target.
    let connection = Connection::open(&catalog_path).expect("open disabled history");
    connection
        .execute(
            "UPDATE activations SET desired_enabled = 0 WHERE skill_id = 'skill-1'",
            [],
        )
        .expect("disable activation");
    let disabled_delete = service
        .plan_delete(&third.agent_id)
        .expect("review after Disable");
    assert!(
        disabled_delete.blocking_activation_skill_ids.is_empty(),
        "disabled history must not be a deletion blocker"
    );
    service
        .apply(&disabled_delete.plan_token)
        .expect("delete configuration after Disable");
    let retained: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM activations WHERE skill_id = 'skill-1' AND desired_enabled = 0",
            [],
            |row| row.get(0),
        )
        .expect("retained history");
    assert_eq!(retained, 1, "keep disabled history and its target root");
    let probe = SqliteCatalogProbe::new();
    assert!(
        probe
            .probe_recovery_profile(&catalog_path)
            .expect("inspect Recovery Profile after deleting the last consumer")
            .required_capabilities,
        "disabled history must not block Home recovery after deleting its Agent"
    );
    // An enabled Activation without a configured target remains invalid.
    connection
        .execute(
            "UPDATE activations SET desired_enabled = 1 WHERE skill_id = 'skill-1'",
            [],
        )
        .expect("simulate an enabled orphan Activation");
    assert!(
        !probe
            .probe_recovery_profile(&catalog_path)
            .expect("inspect invalid Recovery Profile")
            .required_capabilities,
        "enabled orphan Activations must still block Home recovery"
    );
    connection
        .execute(
            "UPDATE activations SET desired_enabled = 0 WHERE skill_id = 'skill-1'",
            [],
        )
        .expect("restore disabled history");
    let current = _sqlite
        .agent_configuration_snapshot()
        .expect("current scan scope");
    assert!(
        current
            .configured_roots()
            .all(|root| root.root_id != root_id),
        "a history-only Root must not remain in the configured scan scope"
    );
    std::fs::remove_dir(&shared_target).expect("remove unused fixture directory");
    let filesystem =
        skill_man_lib::adapters::macos_fs::MacOsFileSystem::new(temp.path().to_path_buf());
    let plan =
        skill_man_lib::core::scan::plan::plan_roots_with_failures(&filesystem, &current).unwrap();
    assert!(
        plan.iter()
            .all(|root| root.configured_path != shared_target && root.plan_error.is_none())
    );
    std::fs::create_dir(&shared_target).unwrap();
    let readded = service
        .plan_create(draft("Readded", &shared_target))
        .unwrap();
    service.apply(&readded.plan_token).unwrap();
    let current = _sqlite.agent_configuration_snapshot().unwrap();
    assert!(current.roots.iter().any(|root| root.root_id == root_id));
}

#[test]
fn recent_project_folders_keep_only_ten_most_recent_entries() {
    let temp = tempfile::tempdir().expect("temp directory");
    let home = temp.path().join("bound-home");
    std::fs::create_dir_all(&home).expect("create home");
    let (_service, sqlite, _gate) = open_service(&home);

    for index in 0..11 {
        sqlite
            .record_recent_project_folder(RecentProjectFolder {
                canonical_path_key: format!("project-{index:02}"),
                canonical_path: PathBuf::from(format!("/project-{index:02}")),
                last_used_at: format!("{index:02}"),
            })
            .expect("record recent project");
    }
    let recent = sqlite
        .list_recent_project_folders()
        .expect("list recent projects");
    assert_eq!(recent.len(), 10);
    assert_eq!(recent[0].canonical_path, PathBuf::from("/project-10"));
    assert!(
        recent
            .iter()
            .all(|folder| folder.canonical_path != Path::new("/project-00"))
    );

    sqlite
        .record_recent_project_folder(RecentProjectFolder {
            canonical_path_key: "project-01".into(),
            canonical_path: PathBuf::from("/project-01"),
            last_used_at: "99".into(),
        })
        .expect("refresh recent project");
    assert_eq!(
        sqlite
            .list_recent_project_folders()
            .expect("list refreshed projects")[0]
            .canonical_path,
        PathBuf::from("/project-01")
    );
    sqlite
        .clear_recent_project_folders()
        .expect("clear recent projects");
    assert!(
        sqlite
            .list_recent_project_folders()
            .expect("list cleared projects")
            .is_empty()
    );
}
