pub mod adapters;
pub mod core;
pub mod seams;
#[path = "tauri/mod.rs"]
pub mod tauri_adapter;

pub fn run() {
    use std::sync::Arc;

    use ::tauri::menu::Menu;
    use ::tauri::tray::TrayIconBuilder;
    use ::tauri::{Listener, Manager, RunEvent, WindowEvent};

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
    use crate::core::preferences::PreferencesService;
    use crate::core::startup::StartupService;
    use crate::core::update::UpdateService;
    use crate::seams::catalog_store::CatalogStore;
    use crate::seams::catalog_store::StartupAccess;
    use crate::seams::preferences_store::PreferencesStore;
    use crate::seams::recovery::RecoveryGate;
    use crate::tauri_adapter::activation_api::ActivationApi;
    use crate::tauri_adapter::adopt_api::AdoptApi;
    use crate::tauri_adapter::catalog_api::CatalogApi;
    use crate::tauri_adapter::commands::{
        activation_conflict_details, apply_activation, apply_activation_replace, apply_adopt,
        apply_file_import, apply_file_import_selection, apply_git_import_selection,
        apply_link_import, apply_relocate_link, apply_skill_updates, cancel_activation,
        cancel_activation_replace, cancel_adopt, cancel_file_import, cancel_git_import_selection,
        cancel_link_import, cancel_relocate_link, check_skill_updates, complete_onboarding,
        create_agent_directory, discover_file_import, discover_file_import_collection,
        discover_git_import, discover_link_import, finalize_activation_replace, finalize_adopt,
        inspect_skill, list_agents, list_skills, load_preferences, pin_skill_updates,
        plan_activation, plan_activation_repair, plan_activation_replace, plan_adopt,
        plan_file_import, plan_file_import_selection, plan_file_reinstall,
        plan_git_import_selection, plan_link_import, plan_skill_updates, relocate_link,
        run_activation_health_check, scan_adopt, startup_info, undo_activation_replace, undo_adopt,
        update_preferences,
    };
    use crate::tauri_adapter::health_api::HealthApi;
    use crate::tauri_adapter::import_api::ImportApi;
    use crate::tauri_adapter::lifecycle::{hide_main_window, show_main_window};
    use crate::tauri_adapter::startup_api::StartupApi;
    use crate::tauri_adapter::tray;
    use crate::tauri_adapter::update_api::UpdateApi;

    let app = ::tauri::Builder::default()
        .plugin(tauri_plugin_autostart::Builder::new().build())
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
                ActivationService::new(runtime_store.clone(), filesystem.clone(), library_root)
                    .with_recovery_gate(recovery_gate)
                    .with_agent_adapters(Arc::new(BuiltInAgentAdapters))
                    .with_conflict_checker(runtime_store.clone()),
            ));
            app.manage(StartupApi::new(
                PreferencesService::new(runtime_store.clone()),
                StartupService::new(
                    runtime_store.clone(),
                    runtime_store.clone(),
                    filesystem.clone(),
                ),
            ));
            // The concrete store is also managed directly so the run loop can
            // refresh the tray without a command round-trip.
            app.manage(runtime_store.clone());

            // Standard macOS app menu so ⌘Q / Quit work (spec §10.3).
            app.set_menu(Menu::default(app.handle())?)?;

            // Apply persisted Preferences at startup: Dock accessory mode and
            // the login item survive restarts (spec §10.2).
            if let Ok(preferences) = runtime_store.load_preferences() {
                if !preferences.show_in_dock {
                    let _ =
                        crate::tauri_adapter::lifecycle::apply_show_in_dock(app.handle(), false);
                }
                if preferences.launch_at_login {
                    let _ =
                        crate::tauri_adapter::lifecycle::apply_launch_at_login(app.handle(), true);
                }
            }

            // Menu-bar tray: resident for the whole app lifetime (spec §9.4).
            let initial_recent = runtime_store
                .recently_enabled(tray::TRAY_SKILL_LIMIT)
                .unwrap_or_default();
            let tray_menu = tray::build_tray_menu(app.handle(), &initial_recent)?;
            let mut tray_builder = TrayIconBuilder::with_id(tray::TRAY_ID).menu(&tray_menu);
            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            } else {
                // macOS template icon: black + alpha, adapts to the menu bar.
                let icon = tauri::image::Image::new(include_bytes!("../icons/tray.rgba"), 18, 18);
                tray_builder = tray_builder.icon(icon).icon_as_template(true);
            }
            tray_builder
                .show_menu_on_left_click(true)
                .on_menu_event(tray::handle_tray_menu_event)
                .build(app)?;

            // Keep the tray's recent list honest after every catalog write.
            let handle = app.handle().clone();
            let listener_handle = handle.clone();
            let tray_store = runtime_store.clone();
            handle.listen(tray::CATALOG_CHANGED_EVENT, move |_| {
                tray::refresh_tray(&listener_handle, tray_store.as_ref());
            });

            Ok(())
        })
        .invoke_handler(::tauri::generate_handler![
            list_skills,
            inspect_skill,
            list_agents,
            run_activation_health_check,
            relocate_link,
            apply_relocate_link,
            cancel_relocate_link,
            plan_activation,
            plan_activation_repair,
            apply_activation,
            cancel_activation,
            activation_conflict_details,
            plan_activation_replace,
            apply_activation_replace,
            cancel_activation_replace,
            undo_activation_replace,
            finalize_activation_replace,
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
            cancel_adopt,
            load_preferences,
            update_preferences,
            startup_info,
            complete_onboarding,
            create_agent_directory
        ])
        .build(::tauri::generate_context!())
        .expect("Skill Man runtime failed");

    app.run(move |app_handle, event| {
        // Tray store for the focus-driven refresh (the catalog-changed
        // listener in setup covers command writes; the run loop covers the
        // rest).
        let tray_store = app_handle.state::<Arc<RuntimeCatalogStore>>();
        match event {
            // Red close button only closes the main window; the app stays
            // resident in the menu bar (spec §9.4).
            RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" => {
                api.prevent_close();
                hide_main_window(app_handle);
            }
            // Dock icon click reopens the window (spec §10.3).
            RunEvent::Reopen { .. } => show_main_window(app_handle),
            // Refreshing the tray on focus keeps the recent list honest even if
            // a catalog event was missed.
            RunEvent::WindowEvent {
                label,
                event: WindowEvent::Focused(true),
                ..
            } if label == "main" => {
                tray::refresh_tray(app_handle, tray_store.as_ref());
            }
            _ => {}
        }
    });
}
