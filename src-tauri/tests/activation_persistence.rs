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

mod common;
use common::BoundTestHome;

#[test]
fn tauri_activation_round_trip_persists_and_reads_back_real_filesystem_state() {
    let home = BoundTestHome::new();
    home.seed_standard_library();
    let library_root = home.library_root.clone();
    let database_path = home.catalog_path();
    let claude_root = home.claude_root();
    std::fs::create_dir_all(&claude_root).expect("create Claude skills directory");

    let target_path = {
        let filesystem = home.filesystem.clone();
        let runtime = home.runtime.clone();
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

    let runtime = home.reopen();
    let filesystem = home.filesystem.clone();
    let catalog = CatalogApi::new(CatalogService::new(runtime.clone()));
    let activation = ActivationApi::new(ActivationService::new(runtime, filesystem, library_root));

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
