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
    use crate::adapters::app_state_store::AppStateStoreFileSystem;
    use crate::adapters::catalog_probe::SqliteCatalogProbe;
    use crate::adapters::closed_catalog::ClosedCatalogStore;
    use crate::adapters::git_source::SystemGitSource;
    use crate::adapters::local_file_source::LocalFileSource;
    use crate::adapters::macos_fs::MacOsFileSystem;
    use crate::adapters::runtime_catalog::RuntimeCatalogStore;
    use crate::adapters::sqlite::SqliteCatalogStore;
    use crate::adapters::system_clock::SystemClock;
    use crate::adapters::tauri_app_updater::TauriAppUpdater;
    use crate::adapters::volume_identity::MacOsVolumeIdentitySource;
    use crate::core::activation::ActivationService;
    use crate::core::adopt::AdoptService;
    use crate::core::app_update::AppUpdateService;
    use crate::core::bootstrap::{
        BootstrapConfig, BootstrapService, BootstrapSnapshot, CatalogAccess,
    };
    use crate::core::catalog::CatalogService;
    use crate::core::import::ImportService;
    use crate::core::maintenance::MaintenanceService;
    use crate::core::preferences::PreferencesService;
    use crate::core::startup::StartupService;
    use crate::core::update::UpdateService;
    use crate::core::write_gate::{ClosedReason, ReadOnlyReason, WriteGate, WriteGateState};
    use crate::seams::activation_store::ActivationStore;
    use crate::seams::adopt_store::AdoptStore;
    use crate::seams::catalog_store::CatalogStore;
    use crate::seams::import_store::ImportStore;
    use crate::seams::maintenance_store::MaintenanceStore;
    use crate::seams::preferences_store::PreferencesStore;
    use crate::tauri_adapter::activation_api::ActivationApi;
    use crate::tauri_adapter::adopt_api::AdoptApi;
    use crate::tauri_adapter::app_update_api::AppUpdateApi;
    use crate::tauri_adapter::bootstrap_api::{BootstrapApi, TauriBootstrapChangedEmitter};
    use crate::tauri_adapter::catalog_api::CatalogApi;
    use crate::tauri_adapter::commands::{
        activation_conflict_details, apply_activation, apply_activation_replace, apply_adopt,
        apply_file_import, apply_file_import_selection, apply_git_import_selection,
        apply_link_import, apply_relocate_link, apply_remove_skill, apply_skill_updates,
        cancel_activation, cancel_activation_replace, cancel_adopt, cancel_app_update,
        cancel_file_import, cancel_git_import_selection, cancel_link_import, cancel_relocate_link,
        cancel_remove_skill, check_app_update, check_skill_updates, complete_onboarding,
        create_agent_directory, discover_file_import, discover_file_import_collection,
        discover_git_import, discover_link_import, download_app_update,
        finalize_activation_replace, finalize_adopt, get_bootstrap_snapshot, inspect_skill,
        install_app_update, list_agents, list_skills, load_preferences, pin_skill_updates,
        plan_activation, plan_activation_repair, plan_activation_replace, plan_adopt,
        plan_file_import, plan_file_import_selection, plan_file_reinstall,
        plan_git_import_selection, plan_link_import, plan_remove_skill, plan_skill_updates,
        relocate_link, run_activation_health_check, scan_adopt, startup_info,
        undo_activation_replace, undo_adopt, update_preferences,
    };
    use crate::tauri_adapter::health_api::HealthApi;
    use crate::tauri_adapter::import_api::ImportApi;
    use crate::tauri_adapter::lifecycle::{hide_main_window, show_main_window};
    use crate::tauri_adapter::startup_api::StartupApi;
    use crate::tauri_adapter::tray;
    use crate::tauri_adapter::update_api::UpdateApi;

    let app = ::tauri::Builder::default()
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let home_directory = app.path().home_dir().map_err(|error| error.to_string())?;
            let state_dir = home_directory.join("Library/Application Support/skill-man-state");
            let default_home_path = home_directory.join("Library/Application Support/skill-man");
            let catalog_file_name = "skill-man.sqlite3".to_string();

            let filesystem = Arc::new(MacOsFileSystem::new(home_directory.clone()));
            let app_state = Arc::new(AppStateStoreFileSystem::new(state_dir.clone()));
            let volume_identity = Arc::new(MacOsVolumeIdentitySource::new());
            let catalog_probe = Arc::new(SqliteCatalogProbe::new());
            let write_gate = Arc::new(WriteGate::new(WriteGateState::Closed {
                reason: ClosedReason::Unconfigured,
            }));
            let bootstrap = Arc::new(BootstrapService::new(
                app_state,
                volume_identity,
                catalog_probe,
                filesystem.clone(),
                BootstrapConfig {
                    state_dir,
                    default_home_path: default_home_path.clone(),
                    catalog_file_name: catalog_file_name.clone(),
                },
            ));

            // Bootstrap authority first (spec §5.1): the snapshot decides
            // whether any catalog opens, and only a four-way-verified Bound
            // Home opens writable. There is no fixture seed or fallback; a
            // failed writable open degrades to a read-only session.
            let snapshot = bootstrap.inspect();
            let bound_home = bootstrap.verified_bound_home();
            // Every write module operates on the verified Bound Home path
            // (spec §3.1: cache/operations/staging belong to the Home); the
            // default path is only the no-binding placeholder and is never
            // created.
            let resolved_library_root = match bound_home.as_ref() {
                Some(home) => home.path.clone(),
                None => default_home_path.clone(),
            };
            let mut gate_state = snapshot.write_gate_state(bound_home.as_ref());
            let runtime_store: Option<Arc<RuntimeCatalogStore>> =
                match (&snapshot, bound_home.as_ref()) {
                    (
                        BootstrapSnapshot::Bound {
                            catalog_access: CatalogAccess::ReadWrite,
                            ..
                        },
                        Some(home),
                    ) => {
                        let path = home.path.join(&catalog_file_name);
                        match SqliteCatalogStore::open_bound(home, &path) {
                            Ok(sqlite) => Some(Arc::new(RuntimeCatalogStore::new(
                                Arc::new(sqlite),
                                filesystem.clone(),
                            ))),
                            Err(error) => {
                                eprintln!(
                                    "[skill-man] bound catalog open failed; continuing read-only: {error}"
                                );
                                bootstrap.note_catalog_open_failure(error.to_string());
                                gate_state = WriteGateState::CatalogReadOnly {
                                    reason: ReadOnlyReason::OpenFailed,
                                };
                                SqliteCatalogStore::open_read_only(&path)
                                    .ok()
                                    .map(|sqlite| {
                                        Arc::new(RuntimeCatalogStore::new(
                                            Arc::new(sqlite),
                                            filesystem.clone(),
                                        ))
                                    })
                            }
                        }
                    }
                    (
                        BootstrapSnapshot::Bound {
                            catalog_access: CatalogAccess::ReadOnly { .. },
                            ..
                        },
                        Some(home),
                    ) => SqliteCatalogStore::open_read_only(&home.path.join(&catalog_file_name))
                        .ok()
                        .map(|sqlite| {
                            Arc::new(RuntimeCatalogStore::new(Arc::new(sqlite), filesystem.clone()))
                        }),
                    _ => None,
                };
            let _ = write_gate.transition_to(gate_state);

            // Every service consumes the real store when Bound, otherwise the
            // closed store: all catalog commands fail closed outside Bound.
            let catalog_store: Arc<dyn CatalogStore> = match runtime_store.clone() {
                Some(store) => store.clone(),
                None => Arc::new(ClosedCatalogStore),
            };
            let import_store: Arc<dyn ImportStore> = match runtime_store.clone() {
                Some(store) => store.clone(),
                None => Arc::new(ClosedCatalogStore),
            };
            let adopt_store: Arc<dyn AdoptStore> = match runtime_store.clone() {
                Some(store) => store.clone(),
                None => Arc::new(ClosedCatalogStore),
            };
            let activation_store: Arc<dyn ActivationStore> = match runtime_store.clone() {
                Some(store) => store.clone(),
                None => Arc::new(ClosedCatalogStore),
            };
            let maintenance_store: Arc<dyn MaintenanceStore> = match runtime_store.clone() {
                Some(store) => store.clone(),
                None => Arc::new(ClosedCatalogStore),
            };
            let preferences_store: Arc<dyn PreferencesStore> = match runtime_store.clone() {
                Some(store) => store.clone(),
                None => Arc::new(ClosedCatalogStore),
            };
            let conflict_checker: Arc<dyn crate::core::activation::ActivationConflictChecker> =
                match runtime_store.clone() {
                    Some(store) => store.clone(),
                    None => Arc::new(ClosedCatalogStore),
                };

            app.manage(BootstrapApi::new(
                bootstrap.clone(),
                write_gate.clone(),
                Arc::new(TauriBootstrapChangedEmitter::new(app.handle().clone())),
            ));
            app.manage(AppUpdateApi::new(AppUpdateService::new(
                Arc::new(TauriAppUpdater::new(app.handle().clone())),
                preferences_store.clone(),
                Arc::new(SystemClock::new()),
            )));
            let git_cache_root = resolved_library_root.join("cache");
            let import_service = ImportService::new(
                import_store.clone(),
                filesystem.clone(),
                Arc::new(SystemClock::new()),
                Arc::new(LocalFileSource::new()),
                resolved_library_root.clone(),
            )
            .with_write_gate(write_gate.clone())
            .with_git_source(Arc::new(SystemGitSource::new()))
            .with_git_cache_root(git_cache_root.clone());
            app.manage(CatalogApi::new(CatalogService::new(catalog_store.clone())));
            app.manage(HealthApi::new(
                MaintenanceService::new(maintenance_store.clone(), filesystem.clone())
                    .with_library_root(resolved_library_root.clone())
                    .with_write_gate(write_gate.clone())
                    .begin_startup(),
            ));
            app.manage(ImportApi::new(import_service));
            app.manage(UpdateApi::new(UpdateService::new(
                import_store.clone(),
                filesystem.clone(),
                Arc::new(SystemClock::new()),
                Arc::new(SystemGitSource::new()),
                resolved_library_root.clone(),
                git_cache_root,
                write_gate.clone(),
            )));
            app.manage(AdoptApi::new(
                AdoptService::new(
                    adopt_store.clone(),
                    filesystem.clone(),
                    Arc::new(SystemClock::new()),
                    resolved_library_root.clone(),
                    home_directory,
                )
                .with_write_gate(write_gate.clone()),
            ));
            app.manage(ActivationApi::new(
                ActivationService::new(
                    activation_store.clone(),
                    filesystem.clone(),
                    resolved_library_root,
                )
                .with_write_gate(write_gate)
                .with_agent_adapters(Arc::new(BuiltInAgentAdapters))
                .with_conflict_checker(conflict_checker),
            ));
            app.manage(StartupApi::new(
                PreferencesService::new(preferences_store.clone()),
                StartupService::new(catalog_store.clone(), adopt_store.clone(), filesystem.clone()),
            ));
            // The concrete store surface is also managed directly so the run
            // loop can refresh the tray without a command round-trip.
            app.manage(catalog_store.clone());

            // Standard macOS app menu so ⌘Q / Quit work (spec §10.3).
            app.set_menu(Menu::default(app.handle())?)?;

            // Apply persisted Preferences at startup: Dock accessory mode and
            // the login item survive restarts (spec §10.2). Closed states
            // have no catalog, so preferences simply stay defaulted.
            if let Ok(preferences) = preferences_store.load_preferences() {
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
            let initial_recent = catalog_store
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
            let tray_store = catalog_store.clone();
            handle.listen(tray::CATALOG_CHANGED_EVENT, move |_| {
                tray::refresh_tray(&listener_handle, tray_store.as_ref());
            });

            Ok(())
        })
        .invoke_handler(::tauri::generate_handler![
            get_bootstrap_snapshot,
            check_app_update,
            download_app_update,
            cancel_app_update,
            install_app_update,
            list_skills,
            inspect_skill,
            list_agents,
            run_activation_health_check,
            relocate_link,
            apply_relocate_link,
            cancel_relocate_link,
            plan_remove_skill,
            apply_remove_skill,
            cancel_remove_skill,
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
        // rest). Absent outside Bound: the tray keeps its last good menu.
        let tray_store = app_handle.try_state::<Arc<dyn CatalogStore>>();
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
                if let Some(store) = tray_store {
                    tray::refresh_tray(app_handle, store.as_ref());
                }
            }
            _ => {}
        }
    });
}
