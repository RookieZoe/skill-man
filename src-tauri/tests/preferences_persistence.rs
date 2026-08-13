//! Preferences (spec §10.2), first-run onboarding state (spec §8.7) and the
//! tray's recently-enabled query (spec §9.4): persistence, defaults, the
//! schema v3→v4 migration, ordering, and the pure tray label formatting.

use std::sync::Arc;

use rusqlite::Connection;

use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::core::domain::Health;
use skill_man_lib::core::preferences::PreferencesService;
use skill_man_lib::core::startup::StartupService;
use skill_man_lib::seams::catalog_store::CatalogStore;
use skill_man_lib::seams::preferences_store::{AppPreferences, PreferenceUpdates};
use skill_man_lib::tauri_adapter::tray::tray_skill_label;

mod common;
use common::BoundTestHome;

fn runtime(home: &BoundTestHome) -> Arc<RuntimeCatalogStore> {
    home.runtime.clone()
}

#[test]
fn preferences_defaults_match_the_spec_and_partial_updates_persist() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let runtime = runtime(&home);
    let service = PreferencesService::new(runtime.clone());

    let defaults = service.load().expect("load Preferences");
    assert_eq!(
        defaults,
        AppPreferences {
            launch_at_login: false,
            show_in_dock: true,
            check_app_updates: true,
            check_skill_updates: true,
        }
    );

    // A partial update changes only the given field.
    let updated = service
        .update(PreferenceUpdates {
            launch_at_login: Some(true),
            ..Default::default()
        })
        .expect("update Preferences");
    assert!(updated.launch_at_login);
    assert!(updated.show_in_dock);
    assert!(updated.check_app_updates);
    assert!(updated.check_skill_updates);

    // The update survives a fresh service over the same store.
    let reloaded = PreferencesService::new(runtime.clone())
        .load()
        .expect("reload Preferences");
    assert!(reloaded.launch_at_login);
    assert!(reloaded.show_in_dock, "untouched fields keep their values");
}

#[test]
fn first_run_flag_flips_after_onboarding_completes() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let runtime = runtime(&home);
    let filesystem = home.filesystem.clone();
    let startup = StartupService::new(runtime.clone(), runtime.clone(), filesystem);

    let info = startup.startup_info().expect("startup info");
    assert!(info.first_run, "a fresh catalog is still a first run");
    assert!(
        info.agents.iter().any(|agent| agent.detected),
        "onboarding lists detected Agent Presets"
    );
    assert!(
        info.agents
            .iter()
            .all(|agent| !agent.skills_path.as_os_str().is_empty())
    );

    startup.complete_onboarding().expect("complete onboarding");

    let info = startup
        .startup_info()
        .expect("startup info after onboarding");
    assert!(!info.first_run, "later launches are not a first run");
}

#[test]
fn recently_enabled_orders_by_last_enable_and_limits() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let runtime = runtime(&home);
    let database_path = home.catalog_path();

    // Seed Activations with distinct enable times directly (epoch seconds).
    let connection = Connection::open(&database_path).expect("open SQLite for seeding");
    let skill_ids: Vec<(String, String)> = connection
        .prepare("SELECT id, directory_name FROM skills ORDER BY directory_name")
        .expect("prepare skill list")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query skills")
        .collect::<rusqlite::Result<_>>()
        .expect("read skills");
    assert!(skill_ids.len() >= 3, "fixture seeds at least three Skills");
    let (first_id, _first_name) = &skill_ids[0];
    let (second_id, second_name) = &skill_ids[1];
    let (_third_id, third_name) = &skill_ids[2];
    for (index, (skill_id, name)) in skill_ids.iter().enumerate() {
        let enabled_at = 1_700_000_000 + index as i64;
        connection
            .execute(
                "INSERT INTO activations (
                    skill_id, agent_id, desired_enabled, expected_entry_path,
                    expected_target_path, observed_state, last_enabled_at, last_checked_at
                 ) VALUES (?1, 'claude-code', 1, ?2, ?3, 'present', ?4, ?4)",
                rusqlite::params![
                    skill_id,
                    format!("/.claude/skills/{name}"),
                    format!("/Library/skills/{name}"),
                    enabled_at.to_string()
                ],
            )
            .expect("seed Activation");
    }
    // The second Skill is also enabled for a second Agent.
    connection
        .execute(
            "INSERT INTO activations (
                skill_id, agent_id, desired_enabled, expected_entry_path,
                expected_target_path, observed_state, last_enabled_at, last_checked_at
             ) VALUES (?1, 'codex', 1, ?2, ?3, 'present', ?4, ?4)",
            rusqlite::params![
                second_id,
                format!("/.codex/skills/{second_name}"),
                format!("/Library/skills/{second_name}"),
                "1700000001"
            ],
        )
        .expect("seed second-Agent Activation");
    drop(connection);

    let recent = runtime
        .recently_enabled(2)
        .expect("recently enabled Skills");
    assert_eq!(
        recent
            .iter()
            .map(|skill| skill.directory_name.as_str())
            .collect::<Vec<_>>(),
        vec![third_name.as_str(), second_name.as_str()],
        "newest enables first, limited to two"
    );
    assert_eq!(recent[0].enabled_agent_count, 1);
    assert_eq!(recent[1].enabled_agent_count, 2);

    let _ = first_id;
}

#[test]
fn tray_labels_show_agent_counts_and_health_suffixes() {
    let healthy = skill_man_lib::core::domain::SkillSummary {
        id: skill_man_lib::core::domain::SkillId("s1".into()),
        directory_name: "media-xray".into(),
        display_name: "Media X-Ray".into(),
        description: String::new(),
        source_kind: skill_man_lib::core::domain::SourceKind::FileInstall,
        health: Health::Healthy,
        enabled_agent_count: 1,
    };
    assert_eq!(tray_skill_label(&healthy), "media-xray · 1 Agent");

    let broken = skill_man_lib::core::domain::SkillSummary {
        enabled_agent_count: 2,
        health: Health::Broken,
        ..healthy.clone()
    };
    assert_eq!(tray_skill_label(&broken), "media-xray · 2 Agents · broken");

    let modified = skill_man_lib::core::domain::SkillSummary {
        enabled_agent_count: 0,
        health: Health::Modified,
        ..healthy.clone()
    };
    assert_eq!(
        tray_skill_label(&modified),
        "media-xray · 0 Agents · modified"
    );
}

#[test]
fn create_agent_directory_only_creates_known_missing_presets() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let runtime = runtime(&home);
    let filesystem = home.filesystem.clone();
    let startup = StartupService::new(runtime.clone(), runtime.clone(), filesystem);

    // An unknown Agent is rejected.
    let error = startup
        .create_agent_directory(&skill_man_lib::core::domain::AgentId("ghost".into()))
        .expect_err("unknown Agent is rejected");
    assert!(matches!(
        error,
        skill_man_lib::core::startup::StartupError::Validation(_)
    ));

    // A detected preset is rejected — nothing is created.
    let error = startup
        .create_agent_directory(&skill_man_lib::core::domain::AgentId("claude-code".into()))
        .expect_err("detected preset is not created again");
    assert!(matches!(
        error,
        skill_man_lib::core::startup::StartupError::Validation(_)
    ));

    // A configured preset with a missing directory is created on request.
    let missing_skills_path = home.path().join(".custom-tools/skills");
    {
        let database_path = home.catalog_path();
        let connection = Connection::open(&database_path).expect("open SQLite");
        connection
            .execute(
                "INSERT INTO agents (
                    id, name, kind, skills_path, path_identity_key, detected,
                    compatibility, created_at, updated_at
                 ) VALUES (?1, ?2, 'custom', ?3, ?4, 0, 'unknown', '0', '0')",
                rusqlite::params![
                    "custom-workbench",
                    "Custom Workbench",
                    missing_skills_path.to_string_lossy(),
                    missing_skills_path.to_string_lossy(),
                ],
            )
            .expect("seed undetected Agent");
    }
    let missing_agent = {
        let agents =
            match skill_man_lib::seams::adopt_store::AdoptStore::list_agents(runtime.as_ref()) {
                Ok(agents) => agents,
                Err(error) => panic!("list agents failed: {error}"),
            };
        let found = agents.iter().find(|agent| {
            agent.agent_id == skill_man_lib::core::domain::AgentId("custom-workbench".into())
        });
        match found {
            Some(agent) => agent.clone(),
            None => panic!(
                "custom-workbench missing from: {:?}",
                agents
                    .iter()
                    .map(|a| a.agent_id.0.clone())
                    .collect::<Vec<_>>()
            ),
        }
    };
    assert!(!missing_agent.detected);
    let skills_path = missing_agent.skills_path.clone();
    assert!(!skills_path.exists());
    startup
        .create_agent_directory(&missing_agent.agent_id)
        .expect("create missing preset directory");
    assert!(
        skills_path.is_dir(),
        "directory was created after confirmation"
    );
    // The Agent is now detected.
    let agents = skill_man_lib::seams::adopt_store::AdoptStore::list_agents(runtime.as_ref())
        .expect("list agents after creation");
    let agent = agents
        .iter()
        .find(|agent| agent.agent_id == missing_agent.agent_id)
        .expect("Agent still configured");
    assert!(agent.detected);
}

#[test]
fn pre_identity_catalogs_open_read_only_with_migration_required() {
    let dir = tempfile::tempdir().expect("temporary home");
    let database_path = dir
        .path()
        .join("Library/Application Support/skill-man.sqlite3");
    std::fs::create_dir_all(database_path.parent().expect("parent directory"))
        .expect("create parent directory");
    let connection = Connection::open(&database_path).expect("create v4 catalog");
    connection
        .execute_batch(
            "CREATE TABLE catalog_meta (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                schema_version INTEGER NOT NULL,
                snapshot_version INTEGER NOT NULL DEFAULT 0,
                first_run_completed_at TEXT,
                last_startup_check_at TEXT
             );
             INSERT INTO catalog_meta (singleton, schema_version, snapshot_version)
             VALUES (1, 4, 0);",
        )
        .expect("write v4 schema");
    drop(connection);

    // Ordinary open() must not migrate pre-identity schemas (spec §3.4).
    let sqlite = SqliteCatalogStore::open(&database_path).expect("open read-only");
    let status = sqlite.startup_status();
    assert_eq!(
        status.access,
        skill_man_lib::seams::catalog_store::StartupAccess::ReadOnly
    );
    assert_eq!(
        status.diagnostic.expect("diagnostic").code,
        skill_man_lib::seams::catalog_store::StartupDiagnosticCode::MigrationRequired
    );
    assert_eq!(
        sqlite.first_run_completed_at().expect("first-run flag"),
        None,
        "a read-only probe does not expose onboarding state"
    );
}
