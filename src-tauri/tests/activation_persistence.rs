use std::sync::Arc;

use skill_man_lib::adapters::fixture_catalog::FixtureCatalogStore;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::core::activation::ActivationService;
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::maintenance::MaintenanceService;
use skill_man_lib::tauri_adapter::activation_api::ActivationApi;
use skill_man_lib::tauri_adapter::catalog_api::CatalogApi;
use skill_man_lib::tauri_adapter::dto::{
    ActivationObservedStateDto, ApplyActivationRequestDto, PlanActivationRepairRequestDto,
    PlanActivationRequestDto,
};
use skill_man_lib::tauri_adapter::health_api::HealthApi;

#[test]
fn tauri_activation_round_trip_persists_and_reads_back_real_filesystem_state() {
    let home = tempfile::tempdir().expect("temporary home");
    let library_root = home.path().join("Library/Application Support/skill-man");
    let database_path = library_root.join("skill-man.sqlite3");
    let claude_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");

    let target_path = {
        let fixture = Arc::new(
            FixtureCatalogStore::runtime(&library_root).expect("materialize runtime fixture"),
        );
        let sqlite = Arc::new(SqliteCatalogStore::open(&database_path).expect("open SQLite"));
        sqlite
            .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
            .expect("seed catalog metadata");
        let runtime = Arc::new(RuntimeCatalogStore::new(fixture, sqlite));
        let filesystem = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
        let catalog = CatalogApi::new(CatalogService::new(runtime.clone()));
        let activation = ActivationApi::new(ActivationService::new(
            runtime.clone(),
            filesystem.clone(),
            library_root.clone(),
        ));

        let before = catalog
            .list_agents("skill-authoring".into())
            .expect("initial Agent state");
        assert!(!before[0].desired_enabled);
        assert_eq!(
            before[0].observed_state,
            ActivationObservedStateDto::Missing
        );

        let preview = activation
            .plan_activation(PlanActivationRequestDto {
                skill_id: "skill-authoring".into(),
                agent_id: "claude-code".into(),
                enabled: true,
            })
            .expect("plan Enable through Tauri adapter");
        let target_path = preview.target_path.clone();
        activation
            .apply_activation(ApplyActivationRequestDto {
                plan_token: preview.plan_token,
            })
            .expect("apply Enable through Tauri adapter");

        assert_eq!(
            std::fs::read_link(claude_root.join("skill-authoring"))
                .expect("real Activation symlink"),
            std::path::PathBuf::from(&target_path)
        );
        let after = catalog
            .list_agents("skill-authoring".into())
            .expect("read back Agent state");
        assert!(after[0].desired_enabled);
        assert_eq!(after[0].observed_state, ActivationObservedStateDto::Present);
        std::fs::remove_file(claude_root.join("skill-authoring"))
            .expect("simulate Claude update deleting the Activation");
        let health = HealthApi::new(MaintenanceService::new(runtime, filesystem).begin_startup());
        let report = health
            .run_activation_health_check()
            .expect("read completed startup Activation health check");
        assert_eq!(report.checked, 1);
        let missing = catalog
            .list_agents("skill-authoring".into())
            .expect("read missing health state");
        assert!(missing[0].desired_enabled);
        assert_eq!(
            missing[0].observed_state,
            ActivationObservedStateDto::Missing
        );
        let sentinel_last_enabled_at = "enable-sentinel";
        let connection = rusqlite::Connection::open(&database_path).expect("inspect SQLite state");
        connection
            .execute(
                "UPDATE activations SET last_enabled_at = ?1
                 WHERE skill_id = 'skill-authoring' AND agent_id = 'claude-code'",
                [sentinel_last_enabled_at],
            )
            .expect("set stable Enable timestamp sentinel");
        drop(connection);
        let occupied_path = claude_root.join("skill-authoring");
        std::fs::create_dir(&occupied_path).expect("occupy missing Activation entry");
        let conflict = activation
            .plan_activation_repair(PlanActivationRepairRequestDto {
                skill_id: "skill-authoring".into(),
                agent_id: "claude-code".into(),
            })
            .expect_err("Repair reports occupied entry");
        assert_eq!(conflict.code, "conflict");
        let occupied = catalog
            .list_agents("skill-authoring".into())
            .expect("read occupied Repair observation");
        assert_eq!(
            occupied[0].observed_state,
            ActivationObservedStateDto::Occupied
        );
        let connection =
            rusqlite::Connection::open(&database_path).expect("reinspect SQLite state");
        let last_enabled_at: String = connection
            .query_row(
                "SELECT last_enabled_at FROM activations
                 WHERE skill_id = 'skill-authoring' AND agent_id = 'claude-code'",
                [],
                |row| row.get(0),
            )
            .expect("read preserved Enable timestamp");
        assert_eq!(last_enabled_at, sentinel_last_enabled_at);
        drop(connection);
        std::fs::remove_dir(&occupied_path).expect("clear occupied entry for Repair");
        health
            .run_activation_health_check()
            .expect("refresh missing state after clearing conflict");
        target_path
    };

    let fixture = Arc::new(
        FixtureCatalogStore::runtime(&library_root).expect("rematerialize runtime fixture"),
    );
    let sqlite = Arc::new(SqliteCatalogStore::open(&database_path).expect("reopen SQLite"));
    sqlite
        .seed_catalog_if_empty(&fixture.catalog_seed().expect("fixture seed"))
        .expect("preserve existing catalog metadata");
    let runtime = Arc::new(RuntimeCatalogStore::new(fixture, sqlite));
    let catalog = CatalogApi::new(CatalogService::new(runtime.clone()));
    let activation = ActivationApi::new(ActivationService::new(
        runtime,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root,
    ));

    let restored = catalog
        .list_agents("skill-authoring".into())
        .expect("read persisted Agent state after restart");
    assert!(restored[0].desired_enabled);
    assert_eq!(
        restored[0].observed_state,
        ActivationObservedStateDto::Missing
    );

    let repair = activation
        .plan_activation_repair(PlanActivationRepairRequestDto {
            skill_id: "skill-authoring".into(),
            agent_id: "claude-code".into(),
        })
        .expect("plan Repair from persisted missing state");
    assert_eq!(repair.target_path, target_path);
    activation
        .apply_activation(ApplyActivationRequestDto {
            plan_token: repair.plan_token,
        })
        .expect("apply Repair from persisted state");
    let repaired = catalog
        .list_agents("skill-authoring".into())
        .expect("read repaired Agent state");
    assert_eq!(
        repaired[0].observed_state,
        ActivationObservedStateDto::Present
    );

    let preview = activation
        .plan_activation(PlanActivationRequestDto {
            skill_id: "skill-authoring".into(),
            agent_id: "claude-code".into(),
            enabled: false,
        })
        .expect("plan Disable from persisted state");
    assert_eq!(preview.target_path, target_path);
    activation
        .apply_activation(ApplyActivationRequestDto {
            plan_token: preview.plan_token,
        })
        .expect("apply Disable from persisted state");

    assert!(!claude_root.join("skill-authoring").exists());
    let disabled = catalog
        .list_agents("skill-authoring".into())
        .expect("read disabled Agent state");
    assert!(!disabled[0].desired_enabled);
    assert_eq!(
        disabled[0].observed_state,
        ActivationObservedStateDto::Missing
    );
}
