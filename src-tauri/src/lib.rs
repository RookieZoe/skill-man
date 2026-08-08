pub mod adapters;
pub mod core;
pub mod seams;
#[path = "tauri/mod.rs"]
pub mod tauri_adapter;

pub fn run() {
    use std::sync::Arc;

    use ::tauri::Manager;

    use crate::adapters::agent_adapters::BuiltInAgentAdapters;
    use crate::adapters::fixture_catalog::FixtureCatalogStore;
    use crate::adapters::local_file_source::LocalFileSource;
    use crate::adapters::macos_fs::MacOsFileSystem;
    use crate::adapters::runtime_catalog::RuntimeCatalogStore;
    use crate::adapters::sqlite::SqliteCatalogStore;
    use crate::adapters::system_clock::SystemClock;
    use crate::core::activation::ActivationService;
    use crate::core::catalog::CatalogService;
    use crate::core::import::ImportService;
    use crate::core::maintenance::MaintenanceService;
    use crate::seams::catalog_store::StartupAccess;
    use crate::seams::recovery::RecoveryGate;
    use crate::tauri_adapter::activation_api::ActivationApi;
    use crate::tauri_adapter::catalog_api::CatalogApi;
    use crate::tauri_adapter::commands::{
        apply_activation, apply_file_import, apply_file_import_selection, apply_link_import,
        cancel_activation, cancel_file_import, cancel_link_import, discover_file_import,
        discover_file_import_collection, discover_link_import, inspect_skill, list_agents,
        list_skills, plan_activation, plan_activation_repair, plan_file_import,
        plan_file_import_selection, plan_file_reinstall, plan_link_import,
        run_activation_health_check,
    };
    use crate::tauri_adapter::health_api::HealthApi;
    use crate::tauri_adapter::import_api::ImportApi;

    ::tauri::Builder::default()
        .setup(|app| {
            let home_directory = app.path().home_dir().map_err(|error| error.to_string())?;
            let library_root = home_directory.join("Library/Application Support/skill-man");
            let sqlite_store = Arc::new(
                SqliteCatalogStore::open(&library_root.join("skill-man.sqlite3"))
                    .map_err(|error| error.to_string())?,
            );
            let fixture_store = if sqlite_store.startup_status().access == StartupAccess::ReadWrite
            {
                let fixture_store = Arc::new(
                    FixtureCatalogStore::runtime(&library_root)
                        .map_err(|error| error.to_string())?,
                );
                sqlite_store
                    .seed_catalog_if_empty(
                        &fixture_store
                            .catalog_seed()
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?;
                fixture_store
            } else {
                Arc::new(FixtureCatalogStore::library_desk())
            };
            let filesystem = Arc::new(MacOsFileSystem::new(home_directory));
            let runtime_store = Arc::new(RuntimeCatalogStore::new(
                fixture_store,
                sqlite_store,
                filesystem.clone(),
            ));
            let recovery_gate = Arc::new(RecoveryGate::blocked());
            app.manage(CatalogApi::new(CatalogService::new(runtime_store.clone())));
            app.manage(HealthApi::new(
                MaintenanceService::new(runtime_store.clone(), filesystem.clone())
                    .with_library_root(library_root.clone())
                    .with_recovery_gate(recovery_gate.clone())
                    .begin_startup(),
            ));
            app.manage(ImportApi::new(
                ImportService::new(
                    runtime_store.clone(),
                    filesystem.clone(),
                    Arc::new(SystemClock::new()),
                    Arc::new(LocalFileSource::new()),
                    library_root.clone(),
                )
                .with_recovery_gate(recovery_gate.clone()),
            ));
            app.manage(ActivationApi::new(
                ActivationService::new(runtime_store, filesystem, library_root)
                    .with_recovery_gate(recovery_gate)
                    .with_agent_adapters(Arc::new(BuiltInAgentAdapters)),
            ));
            Ok(())
        })
        .invoke_handler(::tauri::generate_handler![
            list_skills,
            inspect_skill,
            list_agents,
            run_activation_health_check,
            plan_activation,
            plan_activation_repair,
            apply_activation,
            cancel_activation,
            discover_link_import,
            plan_link_import,
            apply_link_import,
            cancel_link_import,
            discover_file_import,
            discover_file_import_collection,
            plan_file_import,
            plan_file_reinstall,
            plan_file_import_selection,
            apply_file_import,
            apply_file_import_selection,
            cancel_file_import
        ])
        .run(::tauri::generate_context!())
        .expect("Skill Man runtime failed");
}
