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
    use crate::adapters::git_source::SystemGitSource;
    use crate::adapters::local_file_source::LocalFileSource;
    use crate::adapters::macos_fs::MacOsFileSystem;
    use crate::adapters::runtime_catalog::RuntimeCatalogStore;
    use crate::adapters::sqlite::SqliteCatalogStore;
    use crate::adapters::system_clock::SystemClock;
    use crate::core::activation::ActivationService;
    use crate::core::adopt::AdoptService;
    use crate::core::catalog::CatalogService;
    use crate::core::import::ImportService;
    use crate::core::maintenance::MaintenanceService;
    use crate::core::update::UpdateService;
    use crate::seams::catalog_store::StartupAccess;
    use crate::seams::recovery::RecoveryGate;
    use crate::tauri_adapter::activation_api::ActivationApi;
    use crate::tauri_adapter::adopt_api::AdoptApi;
    use crate::tauri_adapter::catalog_api::CatalogApi;
    use crate::tauri_adapter::commands::{
        apply_activation, apply_adopt, apply_file_import, apply_file_import_selection,
        apply_git_import_selection, apply_link_import, apply_skill_updates, cancel_activation,
        cancel_adopt, cancel_file_import, cancel_git_import_selection, cancel_link_import,
        check_skill_updates, discover_file_import, discover_file_import_collection,
        discover_git_import, discover_link_import, finalize_adopt, inspect_skill, list_agents,
        list_skills, pin_skill_updates, plan_activation, plan_activation_repair, plan_adopt,
        plan_file_import, plan_file_import_selection, plan_file_reinstall,
        plan_git_import_selection, plan_link_import, plan_skill_updates,
        run_activation_health_check, scan_adopt, undo_adopt,
    };
    use crate::tauri_adapter::health_api::HealthApi;
    use crate::tauri_adapter::import_api::ImportApi;
    use crate::tauri_adapter::update_api::UpdateApi;

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
            let filesystem = Arc::new(MacOsFileSystem::new(home_directory.clone()));
            let runtime_store = Arc::new(RuntimeCatalogStore::new(
                fixture_store,
                sqlite_store,
                filesystem.clone(),
            ));
            let recovery_gate = Arc::new(RecoveryGate::blocked());
            let git_cache_root = library_root.join("cache");
            let import_service = ImportService::new(
                runtime_store.clone(),
                filesystem.clone(),
                Arc::new(SystemClock::new()),
                Arc::new(LocalFileSource::new()),
                library_root.clone(),
            )
            .with_recovery_gate(recovery_gate.clone())
            .with_git_source(Arc::new(SystemGitSource::new()))
            .with_git_cache_root(git_cache_root.clone());
            app.manage(CatalogApi::new(CatalogService::new(runtime_store.clone())));
            app.manage(HealthApi::new(
                MaintenanceService::new(runtime_store.clone(), filesystem.clone())
                    .with_library_root(library_root.clone())
                    .with_recovery_gate(recovery_gate.clone())
                    .begin_startup(),
            ));
            app.manage(ImportApi::new(import_service));
            app.manage(UpdateApi::new(UpdateService::new(
                runtime_store.clone(),
                filesystem.clone(),
                Arc::new(SystemClock::new()),
                Arc::new(SystemGitSource::new()),
                library_root.clone(),
                git_cache_root,
                recovery_gate.clone(),
            )));
            app.manage(AdoptApi::new(
                AdoptService::new(
                    runtime_store.clone(),
                    filesystem.clone(),
                    Arc::new(SystemClock::new()),
                    library_root.clone(),
                    home_directory,
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
            cancel_file_import,
            discover_git_import,
            plan_git_import_selection,
            apply_git_import_selection,
            cancel_git_import_selection,
            check_skill_updates,
            plan_skill_updates,
            apply_skill_updates,
            pin_skill_updates,
            scan_adopt,
            plan_adopt,
            apply_adopt,
            undo_adopt,
            finalize_adopt,
            cancel_adopt
        ])
        .run(::tauri::generate_context!())
        .expect("Skill Man runtime failed");
}
