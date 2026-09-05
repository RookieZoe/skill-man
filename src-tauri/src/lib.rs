pub mod adapters;
pub mod core;
pub mod seams;
#[path = "tauri/mod.rs"]
pub mod tauri_adapter;

pub fn run() {
    use std::sync::Arc;

    use ::tauri::tray::TrayIconBuilder;
    use ::tauri::{Listener, Manager, RunEvent, WindowEvent};

    use crate::adapters::agent_configuration_fs::MacOsAgentConfigurationFileSystem;
    use crate::adapters::app_state_store::AppStateStoreFileSystem;
    use crate::adapters::catalog_probe::SqliteCatalogProbe;
    use crate::adapters::git_source::SystemGitSource;
    use crate::adapters::git_source_capability::SqliteGitSourceCapabilityReader;
    use crate::adapters::local_file_source::LocalFileSource;
    use crate::adapters::local_git_probe::SystemLocalGitProbe;
    use crate::adapters::locale_store::LocaleStoreFileSystem;
    use crate::adapters::macos_fs::MacOsFileSystem;
    use crate::adapters::remote_provider::SystemRemoteProvider;
    use crate::adapters::runtime_catalog::RuntimeCatalogStore;
    use crate::adapters::runtime_catalog::RuntimeStoreSwitch;
    use crate::adapters::scan_evidence_store::SystemScanEvidenceStoreFactory;
    use crate::adapters::scan_managed_facts::SqliteScanManagedFactsReader;
    use crate::adapters::sqlite::{
        SqliteCatalogStore, SqliteLegacyCatalogMigrator, SqlitePreparedCatalogFactory,
    };
    use crate::adapters::system_clock::SystemClock;
    use crate::adapters::system_installer_lock_store::SystemInstallerLockStore;
    use crate::adapters::system_locale::MacOsSystemLocaleSource;
    use crate::adapters::tauri_app_updater::TauriAppUpdater;
    use crate::adapters::volume_identity::MacOsVolumeIdentitySource;
    use crate::core::adopt::AdoptService;
    use crate::core::agent_configuration::{AgentConfigurationService, PresetRegistry};
    use crate::core::app_update::AppUpdateService;
    use crate::core::bootstrap::{
        BootstrapConfig, BootstrapService, BootstrapSnapshot, CatalogAccess,
    };
    use crate::core::catalog::CatalogService;
    use crate::core::enable::EnableService;
    use crate::core::existing_home_recovery::{
        ExistingHomeRecoveryConfig, ExistingHomeRecoveryService,
    };
    use crate::core::fixture_recovery::{FixtureRecoveryService, SystemFixtureClassifier};
    use crate::core::git_source_capability::GitSourceCapabilityScan;
    use crate::core::home_binding::{HomeBindingConfig, HomeBindingService};
    use crate::core::home_lifecycle::HomeLifecycleService;
    use crate::core::import::ImportService;
    use crate::core::locale::LocaleService;
    use crate::core::maintenance::MaintenanceService;
    use crate::core::observation::ObservationService;
    use crate::core::preferences::PreferencesService;
    use crate::core::scan::ScanCoordinator;
    use crate::core::scan::mutation::ScanMutationCoordinator;
    use crate::core::scan::qualifier::SnapshotDeleteQualifier;
    use crate::core::source_group_preview::SourceGroupPreviewService;
    use crate::core::source_promotion::SourcePromotionService;
    use crate::core::source_transition::SourceTransitionService;
    use crate::core::source_update::SourceUpdateService;
    use crate::core::startup::StartupService;
    use crate::core::update::UpdateService;
    use crate::core::write_gate::{ClosedReason, ReadOnlyReason, WriteGate, WriteGateState};
    use crate::seams::activation_store::ActivationStore;
    use crate::seams::adopt_store::AdoptStore;
    use crate::seams::agent_configuration_store::AgentConfigurationStore;
    use crate::seams::app_state_store::AppStateStore;
    use crate::seams::catalog_store::CatalogStore;
    use crate::seams::import_store::ImportStore;
    use crate::seams::maintenance_store::MaintenanceStore;
    use crate::seams::preferences_store::PreferencesStore;
    use crate::tauri_adapter::adopt_api::AdoptApi;
    use crate::tauri_adapter::agent_configuration_api::AgentConfigurationApi;
    use crate::tauri_adapter::app_update_api::AppUpdateApi;
    use crate::tauri_adapter::bootstrap_api::{BootstrapApi, TauriBootstrapChangedEmitter};
    use crate::tauri_adapter::catalog_api::CatalogApi;
    use crate::tauri_adapter::commands::{
        apply_abandon, apply_adopt, apply_agent_configuration_plan, apply_delete_safety_snapshot,
        apply_file_import, apply_file_import_selection, apply_fixture_recovery,
        apply_global_enable, apply_link_import, apply_project_enable, apply_relocate_link,
        apply_remove_skill, apply_skill_updates, cancel_adopt, cancel_app_update, cancel_candidate,
        cancel_existing_home_recovery, cancel_file_import, cancel_link_import,
        cancel_relocate_link, cancel_remove_skill, cancel_rescan, check_app_update,
        check_skill_updates, clear_recent_project_folders, complete_onboarding,
        confirm_existing_home_recovery, confirm_fixture_recovery_result, confirm_home,
        confirm_source_promotion, confirm_source_transition, confirm_source_update,
        continue_candidate, create_agent_directory, create_local_source_copy, discover_file_import,
        discover_file_import_collection, discover_link_import, download_app_update,
        fetch_latest_and_manage, finalize_adopt, finalize_global_enable, finalize_project_enable,
        finalize_source_promotion, finalize_source_transition, get_agent_management_snapshot,
        get_bootstrap_snapshot, get_fixture_recovery_preview, get_git_source_capability,
        get_locale_snapshot, get_observation_page, get_observation_snapshot, get_scan_report_page,
        inspect_skill, install_app_update, list_recent_project_folders, list_safety_snapshots,
        list_skills, list_target_groups, load_preferences, pin_skill_updates, plan_abandon,
        plan_adopt, plan_create_agent_configuration, plan_delete_agent_configuration,
        plan_delete_safety_snapshot, plan_edit_agent_configuration, plan_file_import,
        plan_file_import_selection, plan_file_reinstall, plan_fixture_recovery, plan_global_enable,
        plan_global_lifecycle, plan_link_import, plan_project_enable, plan_remove_skill,
        plan_restore, plan_skill_updates, prepare_existing_home_recovery, prepare_home,
        preview_source_promotion, preview_source_update, reconnect_same_home,
        refresh_activation_health, refresh_detection, refresh_startup_probe,
        refresh_system_languages, relocate_link, remove_git_source, restore_current_source_release,
        restore_eligibility, run_activation_health_check, set_locale_selection, start_rescan,
        startup_info, undo_adopt, undo_global_enable, undo_project_enable, undo_source_transition,
        update_preferences,
    };
    use crate::tauri_adapter::enable_api::EnableApi;
    use crate::tauri_adapter::existing_home_recovery_api::ExistingHomeRecoveryApi;
    use crate::tauri_adapter::fixture_recovery_api::FixtureRecoveryApi;
    use crate::tauri_adapter::git_source_capability_api::GitSourceCapabilityApi;
    use crate::tauri_adapter::health_api::HealthApi;
    use crate::tauri_adapter::home_binding_api::HomeBindingApi;
    use crate::tauri_adapter::home_lifecycle_api::HomeLifecycleApi;
    use crate::tauri_adapter::import_api::ImportApi;
    use crate::tauri_adapter::lifecycle::{hide_main_window, show_main_window};
    use crate::tauri_adapter::locale_api::{
        LOCALE_CHANGED_EVENT, LocaleApi, TauriLocaleChangedEmitter,
    };
    use crate::tauri_adapter::menu;
    use crate::tauri_adapter::observation_api::{ObservationApi, TauriObservationChangedEmitter};
    use crate::tauri_adapter::source_group_preview_api::SourceGroupPreviewApi;
    use crate::tauri_adapter::source_promotion_api::SourcePromotionApi;
    use crate::tauri_adapter::source_transition_api::SourceTransitionApi;
    use crate::tauri_adapter::source_update_api::SourceUpdateApi;
    use crate::tauri_adapter::startup_api::StartupApi;
    use crate::tauri_adapter::tray;
    use crate::tauri_adapter::update_api::UpdateApi;

    let app = ::tauri::Builder::default()
        .on_menu_event(crate::tauri_adapter::menu::handle_app_menu_event)
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let home_directory = app.path().home_dir().map_err(|error| error.to_string())?;
            let state_dir = home_directory.join("Library/Application Support/skill-man-state");
            let default_home_path = home_directory.join("Library/Application Support/skill-man");
            let catalog_file_name = "skill-man.sqlite3".to_string();

            let filesystem = Arc::new(MacOsFileSystem::new(home_directory.clone()));
            let app_state = Arc::new(AppStateStoreFileSystem::new(state_dir.clone()));
            // Locale authority resolves before any visible surface exists
            // (spec §5.1 step 2, ADR-0011): it is App-level state outside any
            // Home, so it stays readable and writable in every bootstrap
            // state and is never touched by Restore or Abandon.
            let locale_service = Arc::new(LocaleService::new(
                Arc::new(LocaleStoreFileSystem::new(state_dir.clone())),
                Arc::new(MacOsSystemLocaleSource::new(home_directory.clone())),
            ));
            let initial_locale = locale_service.snapshot().effective_locale;
            let volume_identity = Arc::new(MacOsVolumeIdentitySource::new());
            let catalog_probe = Arc::new(SqliteCatalogProbe::new());
            let classifier = Arc::new(SystemFixtureClassifier::new(
                catalog_probe.clone(),
                filesystem.clone(),
                catalog_file_name.clone(),
            ));
            let write_gate = Arc::new(WriteGate::new(WriteGateState::Closed {
                reason: ClosedReason::Unconfigured,
            }));
            let scan_evidence_factory = Arc::new(SystemScanEvidenceStoreFactory);
            let bootstrap = Arc::new(BootstrapService::new(
                app_state.clone(),
                volume_identity.clone(),
                catalog_probe.clone(),
                filesystem.clone(),
                classifier.clone(),
                BootstrapConfig {
                    state_dir: state_dir.clone(),
                    default_home_path: default_home_path.clone(),
                    catalog_file_name: catalog_file_name.clone(),
                },
            ));
            let recovery_service = Arc::new(FixtureRecoveryService::new(
                app_state.clone(),
                catalog_probe.clone(),
                filesystem.clone(),
                Arc::new(SqlitePreparedCatalogFactory),
                bootstrap.clone(),
                write_gate.clone(),
                BootstrapConfig {
                    state_dir: state_dir.clone(),
                    default_home_path: default_home_path.clone(),
                    catalog_file_name: catalog_file_name.clone(),
                },
            )
            .with_delete_qualification(Arc::new(SnapshotDeleteQualifier::new(
                scan_evidence_factory.clone(),
            ))));
            // The one-time Home Binding flow (spec §5.3–§5.4): candidates
            // may not overlap the known Agent skills roots (ADR-0012 §3).
            let home_binding_service = Arc::new(HomeBindingService::new(
                app_state.clone(),
                Arc::new(MacOsVolumeIdentitySource::new()),
                catalog_probe.clone(),
                filesystem.clone(),
                Arc::new(SystemFixtureClassifier::new(
                    catalog_probe.clone(),
                    filesystem.clone(),
                    catalog_file_name.clone(),
                )),
                Arc::new(SqliteLegacyCatalogMigrator),
                Arc::new(SqlitePreparedCatalogFactory),
                bootstrap.clone(),
                write_gate.clone(),
                HomeBindingConfig {
                    state_dir: state_dir.clone(),
                    default_home_path: default_home_path.clone(),
                    catalog_file_name: catalog_file_name.clone(),
                    agent_skill_dirs: vec![
                        home_directory.join(".claude/skills"),
                        home_directory.join(".codex/skills"),
                        home_directory.join("Library/Application Support/workbench/skills"),
                    ],
                },
            ));
            let existing_home_recovery_service = Arc::new(ExistingHomeRecoveryService::new(
                app_state.clone(),
                bootstrap.clone(),
                volume_identity,
                catalog_probe.clone(),
                filesystem.clone(),
                classifier,
                write_gate.clone(),
                ExistingHomeRecoveryConfig {
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
            // A verified writable Home still starts behind the narrow
            // recovery capability. The Catalog may be opened so recovery can
            // inspect it, but no product write is admitted until every
            // durable operation journal has converged.
            if matches!(gate_state, WriteGateState::Open(_)) {
                gate_state = WriteGateState::Recovery {
                    operation_id: "startup-recovery".into(),
                };
            }
            write_gate
                .transition_to(gate_state.clone())
                .map_err(|error| error.to_string())?;
            // The one facade every service holds: the concrete SQLite store
            // when Bound, the fail-closed closed store otherwise. Reconnect,
            // Restore and Abandon swap the inner store through the same
            // facade, so product writes reopen or close without rebuilding
            // the service graph.
            let runtime_store = Arc::new(match (&snapshot, bound_home.as_ref()) {
                (
                    BootstrapSnapshot::Bound {
                        catalog_access: CatalogAccess::ReadWrite,
                        ..
                    },
                    Some(home),
                ) => {
                    let path = home.path.join(&catalog_file_name);
                    match SqliteCatalogStore::open_bound(home, &path) {
                        Ok(sqlite) => RuntimeCatalogStore::new(Arc::new(sqlite), filesystem.clone()),
                        Err(error) => {
                            eprintln!(
                                "[skill-man] bound catalog open failed; continuing read-only: {error}"
                            );
                            bootstrap.note_catalog_open_failure(error.to_string());
                            gate_state = WriteGateState::CatalogReadOnly {
                                reason: ReadOnlyReason::OpenFailed,
                            };
                            match SqliteCatalogStore::open_read_only(&path) {
                                Ok(sqlite) => {
                                    RuntimeCatalogStore::new(Arc::new(sqlite), filesystem.clone())
                                }
                                Err(_) => RuntimeCatalogStore::closed(filesystem.clone()),
                            }
                        }
                    }
                }
                (
                    BootstrapSnapshot::Bound {
                        catalog_access: CatalogAccess::ReadOnly { .. },
                        ..
                    },
                    Some(home),
                ) => match SqliteCatalogStore::open_read_only(&home.path.join(&catalog_file_name)) {
                    Ok(sqlite) => {
                        RuntimeCatalogStore::new(Arc::new(sqlite), filesystem.clone())
                    }
                    Err(_) => RuntimeCatalogStore::closed(filesystem.clone()),
                },
                (
                    BootstrapSnapshot::HomeUnavailable { .. },
                    _,
                ) => {
                    // Spec §5.5: when the Catalog is actually readable, a
                    // HomeUnavailable site still serves it read-only (the
                    // gate keeps every write closed); only a truly
                    // unreachable volume has no Catalog.
                    let path = app_state
                        .load()
                        .ok()
                        .and_then(|files| files.binding.current)
                        .map(|current| current.path.join(&catalog_file_name));
                    match path.and_then(|path| SqliteCatalogStore::open_read_only(&path).ok()) {
                        Some(sqlite) => {
                            RuntimeCatalogStore::new(Arc::new(sqlite), filesystem.clone())
                        }
                        None => RuntimeCatalogStore::closed(filesystem.clone()),
                    }
                }
                _ => RuntimeCatalogStore::closed(filesystem.clone()),
            });
            if write_gate.snapshot().state != gate_state {
                write_gate
                    .transition_to(gate_state)
                    .map_err(|error| error.to_string())?;
            }
            write_gate
                .synchronize_bound_home(bound_home.as_ref())
                .map_err(|error| error.to_string())?;

            // Every service consumes the same facade: the real store when
            // Bound, otherwise the closed store — all catalog commands fail
            // closed outside Bound, and a later Reconnect/Restore swap
            // reaches every service.
            let catalog_store: Arc<dyn CatalogStore> = runtime_store.clone();
            let agent_configuration_store: Arc<dyn AgentConfigurationStore> =
                runtime_store.clone();
            let activation_store: Arc<dyn ActivationStore> = runtime_store.clone();
            let import_store: Arc<dyn ImportStore> = runtime_store.clone();
            let adopt_store: Arc<dyn AdoptStore> = runtime_store.clone();
            let maintenance_store: Arc<dyn MaintenanceStore> = runtime_store.clone();
            let preferences_store: Arc<dyn PreferencesStore> = runtime_store.clone();

            app.manage(BootstrapApi::new(
                bootstrap.clone(),
                write_gate.clone(),
                Arc::new(TauriBootstrapChangedEmitter::new(app.handle().clone())),
            ));
            app.manage(FixtureRecoveryApi::new(
                recovery_service,
                bootstrap.clone(),
                Arc::new(RuntimeStoreSwitch::new(
                    runtime_store.clone(),
                    catalog_file_name.clone(),
                    write_gate.clone(),
                )),
                // A second BootstrapApi instance over the same service and
                // gate; used only to publish `bootstrap://changed` after a
                // recovery commit transitions the top-level route.
                Arc::new(BootstrapApi::new(
                    bootstrap.clone(),
                    write_gate.clone(),
                    Arc::new(TauriBootstrapChangedEmitter::new(app.handle().clone())),
                )),
            ));
            app.manage(HomeBindingApi::new(
                home_binding_service,
                bootstrap.clone(),
                Arc::new(RuntimeStoreSwitch::new(
                    runtime_store.clone(),
                    catalog_file_name.clone(),
                    write_gate.clone(),
                )),
                Arc::new(BootstrapApi::new(
                    bootstrap.clone(),
                    write_gate.clone(),
                    Arc::new(TauriBootstrapChangedEmitter::new(app.handle().clone())),
                )),
            ));
            app.manage(ExistingHomeRecoveryApi::new(
                existing_home_recovery_service,
                bootstrap.clone(),
                Arc::new(RuntimeStoreSwitch::new(
                    runtime_store.clone(),
                    catalog_file_name.clone(),
                    write_gate.clone(),
                )),
                Arc::new(BootstrapApi::new(
                    bootstrap.clone(),
                    write_gate.clone(),
                    Arc::new(TauriBootstrapChangedEmitter::new(app.handle().clone())),
                )),
            ));
            app.manage(HomeLifecycleApi::new(
                Arc::new(HomeLifecycleService::new(
                    app_state.clone(),
                    bootstrap.clone(),
                    write_gate.clone(),
                )),
                bootstrap.clone(),
                Arc::new(RuntimeStoreSwitch::new(
                    runtime_store.clone(),
                    catalog_file_name.clone(),
                    write_gate.clone(),
                )),
                Arc::new(BootstrapApi::new(
                    bootstrap.clone(),
                    write_gate.clone(),
                    Arc::new(TauriBootstrapChangedEmitter::new(app.handle().clone())),
                )),
            ));
            app.manage(LocaleApi::new(
                (*locale_service).clone(),
                Arc::new(TauriLocaleChangedEmitter::new(app.handle().clone())),
            ));
            // The service itself is managed so the run loop and listeners can
            // resolve the current locale without a command round-trip.
            app.manage(locale_service.clone());
            app.manage(AppUpdateApi::new(AppUpdateService::new(
                Arc::new(TauriAppUpdater::new(app.handle().clone())),
                preferences_store.clone(),
                Arc::new(SystemClock::new()),
                write_gate.clone(),
            )));
            let git_cache_root = resolved_library_root.join("cache");
            let import_service = ImportService::new(
                import_store.clone(),
                filesystem.clone(),
                Arc::new(SystemClock::new()),
                Arc::new(LocalFileSource::new()),
                resolved_library_root.clone(),
                write_gate.clone(),
            )
            .with_home_context(write_gate.clone())
            .with_git_source(Arc::new(SystemGitSource::new()))
            .with_git_cache_root(git_cache_root.clone());
            app.manage(CatalogApi::new(CatalogService::new(catalog_store.clone())));
            let agent_configuration_filesystem = Arc::new(
                MacOsAgentConfigurationFileSystem::new(home_directory.clone()),
            );
            let presets = PresetRegistry::system();
            app.manage(AgentConfigurationApi::new(Arc::new(
                AgentConfigurationService::new(
                    agent_configuration_store.clone(),
                    agent_configuration_filesystem.clone(),
                    write_gate.clone(),
                    presets.clone(),
                ),
            )));
            // Observation and Scan Module (#81 + #82): Agent Detection stays
            // zero-write; the full Rescan (manual/onboarding) is streamed to
            // the Home's Scan Evidence Store with the single-flight Run
            // lifecycle (spec §4.10; ADR-0020). Library Desk stays
            // interactive immediately; startup never triggers a full Rescan.
            let scan_mutation = Arc::new(ScanMutationCoordinator::new());
            app.manage(scan_mutation.clone());
            let scan_coordinator = Arc::new(ScanCoordinator::new(
                scan_evidence_factory.clone(),
                filesystem.clone(),
                Arc::new(SystemInstallerLockStore::new(home_directory.clone())),
                Arc::new(SystemLocalGitProbe),
                write_gate.clone(),
                agent_configuration_store.clone(),
                scan_mutation.clone(),
                app_state.clone(),
                Arc::new(SqliteScanManagedFactsReader::new(
                    write_gate.clone(),
                    catalog_file_name.clone(),
                    filesystem.clone(),
                )),
                state_dir.clone(),
                Arc::new(SystemClock::new()),
            ));
            let observation_api = Arc::new(ObservationApi::new(
                Arc::new(ObservationService::new(
                    agent_configuration_filesystem.clone(),
                    filesystem.clone(),
                    presets,
                    write_gate.clone(),
                    agent_configuration_store.clone(),
                    activation_store,
                    Arc::new(SystemClock::new()),
                )
                .with_scan(scan_coordinator.clone())),
                Arc::new(TauriObservationChangedEmitter::new(app.handle().clone())),
            ));
            scan_coordinator.set_observer(observation_api.clone());
            observation_api
                .service_handle()
                .set_observation_observer(observation_api.clone());
            app.manage(observation_api.clone());
            let git_source_capability_scan = Arc::new(GitSourceCapabilityScan::new(Arc::new(
                SqliteGitSourceCapabilityReader::new(
                    write_gate.clone(),
                    catalog_file_name.clone(),
                    filesystem.clone(),
                ),
            )));
            app.manage(GitSourceCapabilityApi::new(git_source_capability_scan.clone()));
            let source_group_preview = Arc::new(SourceGroupPreviewService::new(
                Arc::new(SystemGitSource::new()),
                Arc::new(SystemInstallerLockStore::new(home_directory.clone())),
            )
            .with_remote_provider(Arc::new(SystemRemoteProvider::new(Arc::new(
                SystemGitSource::new(),
            )))));
            let source_transition = Arc::new(
                SourceTransitionService::new(
                    source_group_preview.clone(),
                    Arc::new(SystemGitSource::new()),
                    Arc::new(SystemInstallerLockStore::new(home_directory.clone())),
                    runtime_store.clone(),
                    filesystem.clone(),
                    Arc::new(SystemClock::new()),
                    resolved_library_root.clone(),
                    home_directory.clone(),
                    write_gate.clone(),
                )
                .with_home_context(write_gate.clone())
                .with_promotion_store(runtime_store.clone())
                .with_update_store(runtime_store.clone()),
            );
            let source_promotion = Arc::new(SourcePromotionService::new(
                source_group_preview.clone(),
                runtime_store.clone(),
                source_transition.clone(),
            ));
            let source_update = Arc::new(
                SourceUpdateService::new(
                    source_group_preview.clone(),
                    source_transition.clone(),
                    filesystem.clone(),
                    resolved_library_root.clone(),
                )
                .with_home_context(),
            );
            app.manage(SourceGroupPreviewApi::new(source_group_preview));
            app.manage(SourcePromotionApi::new(source_promotion.clone()));
            app.manage(SourceTransitionApi::new(source_transition.clone()));
            app.manage(SourceUpdateApi::new(source_update.clone()));
            let enable_service = EnableService::new(
                runtime_store.clone(),
                runtime_store.clone(),
                agent_configuration_store.clone(),
                agent_configuration_filesystem.clone(),
                filesystem.clone(),
                Arc::new(SystemClock::new()),
                resolved_library_root.clone(),
                write_gate.clone(),
            )
            .with_home_context(write_gate.clone())
            .with_source_update(source_update.clone());
            app.manage(EnableApi::new(enable_service));
            let source_lifecycle = Arc::new(
                crate::core::source_lifecycle::SourceLifecycleService::new(
                    source_transition.clone(),
                    filesystem.clone(),
                    Arc::new(SystemGitSource::new()),
                    Arc::new(SystemClock::new()),
                    resolved_library_root.clone(),
                )
                .with_home_context()
                .with_app_state_dir(state_dir.clone()),
            );
            app.manage(crate::tauri_adapter::source_lifecycle_api::SourceLifecycleApi::new(
                source_lifecycle.clone(),
            ));
            let startup_maintenance = MaintenanceService::new(
                maintenance_store.clone(),
                filesystem.clone(),
                write_gate.clone(),
                crate::core::maintenance::StartupRecoveryServices::new(
                    source_transition.clone(),
                    source_update.clone(),
                    source_lifecycle.clone(),
                    scan_evidence_factory.clone(),
                ),
            )
            .with_library_root(resolved_library_root.clone())
            .with_home_context(write_gate.clone())
            .begin_startup();
            // Priority at startup: Activation health, Startup Probe, Agent
            // Detection (ADR-0020). All three are asynchronous, but none may
            // observe or write while operation recovery still owns the gate.
            let startup_observation = observation_api.clone();
            startup_maintenance.after_startup_recovery(move || {
                let health_trigger = startup_observation.clone();
                std::thread::spawn(move || {
                    let _ = health_trigger.refresh_activation_health(None);
                });
                let probe_trigger = startup_observation.clone();
                std::thread::spawn(move || {
                    let _ = probe_trigger.refresh_startup_probe();
                });
                std::thread::spawn(move || {
                    let _ = startup_observation.refresh_detection();
                });
            });
            app.manage(HealthApi::new(startup_maintenance));
            app.manage(ImportApi::new(import_service));
            app.manage(UpdateApi::new(UpdateService::new(
                import_store.clone(),
                filesystem.clone(),
                Arc::new(SystemClock::new()),
                Arc::new(SystemGitSource::new()),
                resolved_library_root.clone(),
                git_cache_root,
                write_gate.clone(),
            )
            .with_home_context(write_gate.clone())
            .with_git_source_capability_scan(git_source_capability_scan)));
            app.manage(AdoptApi::new(
                AdoptService::new(
                    adopt_store.clone(),
                    filesystem.clone(),
                    Arc::new(SystemClock::new()),
                    resolved_library_root.clone(),
                    home_directory.clone(),
                    write_gate.clone(),
                )
                .with_home_context(write_gate.clone())
                // The Adopt plan surface consumes the terminal Scan Report
                // through the Observation and Scan Module (spec §4.6); no
                // in-memory report and no remote verification is owned here.
                .with_scan_coordinator(scan_coordinator.clone()),
            ));
            app.manage(StartupApi::new(
                PreferencesService::new(preferences_store.clone(), write_gate.clone()),
                StartupService::new(
                    catalog_store.clone(),
                    adopt_store.clone(),
                    filesystem.clone(),
                    write_gate.clone(),
                )
                .with_home_context(write_gate.clone()),
            ));
            // The concrete store surface is also managed directly so the run
            // loop can refresh the tray without a command round-trip.
            app.manage(catalog_store.clone());

            // Standard macOS app menu so ⌘Q / Quit work (spec §10.3), with
            // Window/Help submenu titles in the effective locale (ADR-0011).
            menu::apply_app_menu(app.handle(), initial_locale);

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
            let tray_menu = tray::build_tray_menu(app.handle(), &initial_recent, initial_locale)?;
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
            let listener_locale = locale_service.clone();
            handle.listen(tray::CATALOG_CHANGED_EVENT, move |_| {
                let locale = listener_locale.snapshot().effective_locale;
                tray::refresh_tray(&listener_handle, tray_store.as_ref(), locale);
            });

            // The locale changed: React, tray and the native menu follow the
            // same generation (ADR-0011). The catalog is never reloaded and
            // no sheet is remounted — only presentation surfaces refresh.
            let locale_menu_handle = app.handle().clone();
            let locale_listener_handle = app.handle().clone();
            let locale_tray_store = catalog_store.clone();
            let locale_service_for_event = locale_service.clone();
            handle.listen(LOCALE_CHANGED_EVENT, move |_| {
                let locale = locale_service_for_event.snapshot().effective_locale;
                menu::apply_app_menu(&locale_menu_handle, locale);
                tray::refresh_tray(&locale_listener_handle, locale_tray_store.as_ref(), locale);
            });

            Ok(())
        })
        .invoke_handler(::tauri::generate_handler![
            prepare_home,
            prepare_existing_home_recovery,
            cancel_existing_home_recovery,
            confirm_existing_home_recovery,
            confirm_home,
            continue_candidate,
            cancel_candidate,
            reconnect_same_home,
            plan_abandon,
            apply_abandon,
            get_bootstrap_snapshot,
            get_git_source_capability,
            fetch_latest_and_manage,
            preview_source_promotion,
            confirm_source_promotion,
            finalize_source_promotion,
            preview_source_update,
            confirm_source_update,
            confirm_source_transition,
            undo_source_transition,
            finalize_source_transition,
            restore_current_source_release,
            create_local_source_copy,
            remove_git_source,
            get_locale_snapshot,
            set_locale_selection,
            refresh_system_languages,
            check_app_update,
            download_app_update,
            cancel_app_update,
            install_app_update,
            list_skills,
            inspect_skill,
            get_agent_management_snapshot,
            get_observation_snapshot,
            refresh_detection,
            refresh_startup_probe,
            refresh_activation_health,
            get_observation_page,
            start_rescan,
            cancel_rescan,
            get_scan_report_page,
            plan_create_agent_configuration,
            plan_edit_agent_configuration,
            plan_delete_agent_configuration,
            apply_agent_configuration_plan,
            run_activation_health_check,
            relocate_link,
            apply_relocate_link,
            cancel_relocate_link,
            plan_remove_skill,
            apply_remove_skill,
            cancel_remove_skill,
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
            check_skill_updates,
            plan_skill_updates,
            apply_skill_updates,
            pin_skill_updates,
            plan_adopt,
            apply_adopt,
            undo_adopt,
            finalize_adopt,
            cancel_adopt,
            list_target_groups,
            plan_global_enable,
            plan_global_lifecycle,
            apply_global_enable,
            undo_global_enable,
            finalize_global_enable,
            list_recent_project_folders,
            clear_recent_project_folders,
            plan_project_enable,
            apply_project_enable,
            undo_project_enable,
            finalize_project_enable,
            load_preferences,
            update_preferences,
            startup_info,
            complete_onboarding,
            create_agent_directory,
            get_fixture_recovery_preview,
            restore_eligibility,
            plan_restore,
            plan_fixture_recovery,
            apply_fixture_recovery,
            confirm_fixture_recovery_result,
            list_safety_snapshots,
            plan_delete_safety_snapshot,
            apply_delete_safety_snapshot
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
            // Dock icon click reopens the window (spec §10.3) and, in System
            // mode, re-negotiates the effective locale from the current
            // preferred-language list (ADR-0011); the LocaleApi emits
            // `locale://changed` when the locale actually changed.
            RunEvent::Reopen { .. } => {
                if let Some(api) = app_handle.try_state::<LocaleApi>() {
                    let _ = api.refresh_system_languages();
                }
                show_main_window(app_handle);
            }
            // Refreshing the tray on focus keeps the recent list honest even if
            // a catalog event was missed.
            RunEvent::WindowEvent {
                label,
                event: WindowEvent::Focused(true),
                ..
            } if label == "main" => {
                if let Some(store) = tray_store {
                    let locale = app_handle
                        .try_state::<Arc<LocaleService>>()
                        .map(|service| service.snapshot().effective_locale)
                        .unwrap_or(crate::seams::locale_store::EffectiveLocale::En);
                    tray::refresh_tray(app_handle, store.as_ref(), locale);
                }
            }
            _ => {}
        }
    });
}
