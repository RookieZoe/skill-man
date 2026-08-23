use tauri::{AppHandle, Emitter, State};

use crate::tauri_adapter::activation_api::ActivationApi;
use crate::tauri_adapter::adopt_api::AdoptApi;
use crate::tauri_adapter::app_update_api::AppUpdateApi;
use crate::tauri_adapter::bootstrap_api::BootstrapApi;
use crate::tauri_adapter::catalog_api::CatalogApi;
use crate::tauri_adapter::dto::{
    AbandonPreviewDto, ActivationConflictDetailsDto, ActivationConflictRequestDto,
    ActivationHealthReportDto, ActivationPreviewDto, ActivationReplacePreviewDto,
    ActivationReplaceUndoResultDto, ActivationResultDto, AdoptEvidenceReportDto, AdoptPlanDto,
    AdoptResultDto, AdoptUndoResultDto, AgentActivationDto, AppPreferencesDto, AppUpdateCheckDto,
    ApplyAbandonRequestDto, ApplyActivationReplaceRequestDto, ApplyActivationRequestDto,
    ApplyAdoptRequestDto, ApplyFileImportRequestDto, ApplyFileImportSelectionRequestDto,
    ApplyFixtureRecoveryRequestDto, ApplyGitImportSelectionRequestDto, ApplyLinkImportRequestDto,
    ApplyRelocateLinkRequestDto, ApplyRemoveSkillRequestDto, ApplySkillUpdatesRequestDto,
    BootstrapSnapshotDto, CancelActivationReplaceRequestDto, CancelActivationRequestDto,
    CancelAdoptRequestDto, CancelAppUpdateRequestDto, CancelFileImportRequestDto,
    CancelGitImportSelectionRequestDto, CancelLinkImportRequestDto, CancelRelocateLinkRequestDto,
    CancelRemoveSkillRequestDto, CancelledAppUpdateDto, CandidateOperationRequestDto,
    CatalogListDto, CheckAppUpdateRequestDto, CheckSkillUpdatesRequestDto, CommandFailureDto,
    ConfirmFixtureRecoveryRequestDto, ConfirmHomeRequestDto, CreateAgentDirectoryRequestDto,
    DeleteSafetySnapshotRequestDto, DeleteSnapshotPreviewDto,
    DiscoverFileImportCollectionRequestDto, DiscoverFileImportRequestDto,
    DiscoverGitImportRequestDto, DiscoverLinkImportRequestDto, DownloadAppUpdateRequestDto,
    DownloadedAppUpdateDto, FileImportCandidateDto, FileImportDiscoveryDto, FileImportPreviewDto,
    FileImportResultDto, FileImportSelectionPreviewDto, FileImportSelectionResultDto,
    FinalizeActivationReplaceRequestDto, FinalizeAdoptRequestDto, FixtureRecoveryPlanDto,
    FixtureRecoveryPreviewDto, GitImportDiscoveryDto, GitImportSelectionPreviewDto,
    GitImportSelectionResultDto, GitSourceCapabilityReportDto, HomeCandidateDto,
    InstallAppUpdateRequestDto, LinkImportCandidateDto, LinkImportPreviewDto, LinkImportResultDto,
    ListSkillsRequestDto, LocaleSnapshotDto, PinSkillUpdatesRequestDto,
    PlanActivationRepairRequestDto, PlanActivationReplaceRequestDto, PlanActivationRequestDto,
    PlanAdoptRequestDto, PlanFileImportRequestDto, PlanFileImportSelectionRequestDto,
    PlanFileReinstallRequestDto, PlanFixtureRecoveryRequestDto, PlanGitImportSelectionRequestDto,
    PlanLinkImportRequestDto, PlanRemoveSkillRequestDto, PlanSkillUpdatesRequestDto,
    PreferenceUpdatesDto, PreferencesWarningDto, PrepareHomeRequestDto, RecoveryResultDto,
    RelocateLinkPreviewDto, RelocateLinkRequestDto, RelocateLinkResultDto, RemoveSkillPreviewDto,
    RemoveSkillResultDto, RestoreEligibilityDto, SafetySnapshotDto, SetLocaleSelectionRequestDto,
    SkillDetailDto, StartupInfoDto, UndoActivationReplaceRequestDto, UndoAdoptRequestDto,
    UpdateCheckReportDto, UpdatePlanDto, UpdatePreferencesResultDto, UpdateResultDto,
};
use crate::tauri_adapter::fixture_recovery_api::FixtureRecoveryApi;
use crate::tauri_adapter::git_source_capability_api::GitSourceCapabilityApi;
use crate::tauri_adapter::health_api::HealthApi;
use crate::tauri_adapter::home_binding_api::HomeBindingApi;
use crate::tauri_adapter::home_lifecycle_api::HomeLifecycleApi;
use crate::tauri_adapter::import_api::ImportApi;
use crate::tauri_adapter::locale_api::LocaleApi;
use crate::tauri_adapter::startup_api::StartupApi;
use crate::tauri_adapter::update_api::UpdateApi;
use crate::tauri_adapter::{lifecycle, tray};

#[tauri::command]
pub fn prepare_home(
    state: State<'_, HomeBindingApi>,
    request: PrepareHomeRequestDto,
) -> Result<HomeCandidateDto, CommandFailureDto> {
    state.prepare_home(request)
}

#[tauri::command]
pub fn confirm_home(
    state: State<'_, HomeBindingApi>,
    request: ConfirmHomeRequestDto,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    state.confirm_home(request)
}

#[tauri::command]
pub fn continue_candidate(
    state: State<'_, HomeBindingApi>,
    request: CandidateOperationRequestDto,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    state.continue_candidate(request)
}

#[tauri::command]
pub fn cancel_candidate(
    state: State<'_, HomeBindingApi>,
    request: CandidateOperationRequestDto,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    state.cancel_candidate(request)
}

#[tauri::command]
pub fn reconnect_same_home(
    state: State<'_, HomeLifecycleApi>,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    state.reconnect_same_home()
}

#[tauri::command]
pub fn plan_abandon(
    state: State<'_, HomeLifecycleApi>,
) -> Result<AbandonPreviewDto, CommandFailureDto> {
    state.plan_abandon()
}

#[tauri::command]
pub fn apply_abandon(
    state: State<'_, HomeLifecycleApi>,
    request: ApplyAbandonRequestDto,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    state.apply_abandon(&request)
}

#[tauri::command]
pub fn get_bootstrap_snapshot(
    state: State<'_, BootstrapApi>,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    state.get_bootstrap_snapshot()
}

#[tauri::command]
pub fn get_git_source_capability(
    state: State<'_, GitSourceCapabilityApi>,
) -> Result<GitSourceCapabilityReportDto, CommandFailureDto> {
    state.get_git_source_capability()
}

#[tauri::command]
pub fn get_locale_snapshot(
    state: State<'_, LocaleApi>,
) -> Result<LocaleSnapshotDto, CommandFailureDto> {
    state.get_locale_snapshot()
}

#[tauri::command]
pub fn set_locale_selection(
    state: State<'_, LocaleApi>,
    request: SetLocaleSelectionRequestDto,
) -> Result<LocaleSnapshotDto, CommandFailureDto> {
    state.set_locale_selection(request)
}

#[tauri::command]
pub fn refresh_system_languages(
    state: State<'_, LocaleApi>,
) -> Result<LocaleSnapshotDto, CommandFailureDto> {
    state.refresh_system_languages()
}

#[tauri::command]
pub async fn check_app_update(
    state: State<'_, AppUpdateApi>,
    request: CheckAppUpdateRequestDto,
) -> Result<AppUpdateCheckDto, CommandFailureDto> {
    state.check_app_update(request).await
}

#[tauri::command]
pub async fn download_app_update(
    state: State<'_, AppUpdateApi>,
    request: DownloadAppUpdateRequestDto,
) -> Result<DownloadedAppUpdateDto, CommandFailureDto> {
    state.download_app_update(request).await
}

#[tauri::command]
pub fn cancel_app_update(
    state: State<'_, AppUpdateApi>,
    request: CancelAppUpdateRequestDto,
) -> Result<CancelledAppUpdateDto, CommandFailureDto> {
    state.cancel_app_update(request)
}

#[tauri::command]
pub fn install_app_update(
    state: State<'_, AppUpdateApi>,
    request: InstallAppUpdateRequestDto,
) -> Result<(), CommandFailureDto> {
    state.install_app_update(request)
}

#[tauri::command]
pub async fn plan_activation(
    state: State<'_, ActivationApi>,
    request: PlanActivationRequestDto,
) -> Result<ActivationPreviewDto, CommandFailureDto> {
    state.plan_activation(request)
}

#[tauri::command]
pub async fn plan_activation_repair(
    state: State<'_, ActivationApi>,
    request: PlanActivationRepairRequestDto,
) -> Result<ActivationPreviewDto, CommandFailureDto> {
    state.plan_activation_repair(request)
}

#[tauri::command]
pub async fn run_activation_health_check(
    app: AppHandle,
    state: State<'_, HealthApi>,
) -> Result<ActivationHealthReportDto, CommandFailureDto> {
    let result = state.run_activation_health_check()?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub async fn relocate_link(
    state: State<'_, HealthApi>,
    request: RelocateLinkRequestDto,
) -> Result<RelocateLinkPreviewDto, CommandFailureDto> {
    state.relocate_link(request)
}

#[tauri::command]
pub async fn apply_relocate_link(
    app: AppHandle,
    state: State<'_, HealthApi>,
    request: ApplyRelocateLinkRequestDto,
) -> Result<RelocateLinkResultDto, CommandFailureDto> {
    let result = state.apply_relocate_link(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn cancel_relocate_link(
    state: State<'_, HealthApi>,
    request: CancelRelocateLinkRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_relocate_link(request)
}

#[tauri::command]
pub async fn plan_remove_skill(
    state: State<'_, HealthApi>,
    request: PlanRemoveSkillRequestDto,
) -> Result<RemoveSkillPreviewDto, CommandFailureDto> {
    state.plan_remove_skill(request)
}

#[tauri::command]
pub async fn apply_remove_skill(
    app: AppHandle,
    state: State<'_, HealthApi>,
    request: ApplyRemoveSkillRequestDto,
) -> Result<RemoveSkillResultDto, CommandFailureDto> {
    let result = state.apply_remove_skill(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn cancel_remove_skill(
    state: State<'_, HealthApi>,
    request: CancelRemoveSkillRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_remove_skill(request)
}

#[tauri::command]
pub async fn apply_activation(
    app: AppHandle,
    state: State<'_, ActivationApi>,
    request: ApplyActivationRequestDto,
) -> Result<ActivationResultDto, CommandFailureDto> {
    let result = state.apply_activation(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[::tauri::command]
pub fn cancel_activation(
    state: State<'_, ActivationApi>,
    request: CancelActivationRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_activation(request)
}
#[tauri::command]
pub fn activation_conflict_details(
    state: State<'_, ActivationApi>,
    request: ActivationConflictRequestDto,
) -> Result<ActivationConflictDetailsDto, CommandFailureDto> {
    state.activation_conflict_details(request)
}

#[tauri::command]
pub async fn plan_activation_replace(
    state: State<'_, ActivationApi>,
    request: PlanActivationReplaceRequestDto,
) -> Result<ActivationReplacePreviewDto, CommandFailureDto> {
    state.plan_activation_replace(request)
}

#[tauri::command]
pub async fn apply_activation_replace(
    app: AppHandle,
    state: State<'_, ActivationApi>,
    request: ApplyActivationReplaceRequestDto,
) -> Result<ActivationResultDto, CommandFailureDto> {
    let result = state.apply_activation_replace(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn cancel_activation_replace(
    state: State<'_, ActivationApi>,
    request: CancelActivationReplaceRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_activation_replace(request)
}

#[tauri::command]
pub async fn undo_activation_replace(
    app: AppHandle,
    state: State<'_, ActivationApi>,
    request: UndoActivationReplaceRequestDto,
) -> Result<ActivationReplaceUndoResultDto, CommandFailureDto> {
    let result = state.undo_activation_replace(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn finalize_activation_replace(
    state: State<'_, ActivationApi>,
    request: FinalizeActivationReplaceRequestDto,
) -> Result<(), CommandFailureDto> {
    state.finalize_activation_replace(request)
}

#[tauri::command]
pub fn list_skills(
    state: State<'_, CatalogApi>,
    request: ListSkillsRequestDto,
) -> Result<CatalogListDto, CommandFailureDto> {
    state.list_skills(request)
}

#[tauri::command]
pub fn inspect_skill(
    state: State<'_, CatalogApi>,
    skill_id: String,
) -> Result<SkillDetailDto, CommandFailureDto> {
    state.inspect_skill(skill_id)
}

#[tauri::command]
pub fn list_agents(
    state: State<'_, CatalogApi>,
    skill_id: String,
) -> Result<Vec<AgentActivationDto>, CommandFailureDto> {
    state.list_agents(skill_id)
}

#[tauri::command]
pub async fn discover_link_import(
    state: State<'_, ImportApi>,
    request: DiscoverLinkImportRequestDto,
) -> Result<LinkImportCandidateDto, CommandFailureDto> {
    state.discover_link_import(request)
}

#[tauri::command]
pub async fn plan_link_import(
    state: State<'_, ImportApi>,
    request: PlanLinkImportRequestDto,
) -> Result<LinkImportPreviewDto, CommandFailureDto> {
    state.plan_link_import(request)
}

#[tauri::command]
pub async fn apply_link_import(
    state: State<'_, ImportApi>,
    request: ApplyLinkImportRequestDto,
) -> Result<LinkImportResultDto, CommandFailureDto> {
    state.apply_link_import(request)
}

#[tauri::command]
pub fn cancel_link_import(
    state: State<'_, ImportApi>,
    request: CancelLinkImportRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_link_import(request)
}

#[tauri::command]
pub async fn discover_file_import(
    state: State<'_, ImportApi>,
    request: DiscoverFileImportRequestDto,
) -> Result<FileImportCandidateDto, CommandFailureDto> {
    state.discover_file_import(request)
}

#[tauri::command]
pub async fn discover_file_import_collection(
    state: State<'_, ImportApi>,
    request: DiscoverFileImportCollectionRequestDto,
) -> Result<FileImportDiscoveryDto, CommandFailureDto> {
    state.discover_file_import_collection(request)
}

#[tauri::command]
pub async fn plan_file_import(
    state: State<'_, ImportApi>,
    request: PlanFileImportRequestDto,
) -> Result<FileImportPreviewDto, CommandFailureDto> {
    state.plan_file_import(request)
}

#[tauri::command]
pub async fn plan_file_reinstall(
    state: State<'_, ImportApi>,
    request: PlanFileReinstallRequestDto,
) -> Result<FileImportPreviewDto, CommandFailureDto> {
    state.plan_file_reinstall(request)
}

#[tauri::command]
pub async fn plan_file_import_selection(
    state: State<'_, ImportApi>,
    request: PlanFileImportSelectionRequestDto,
) -> Result<FileImportSelectionPreviewDto, CommandFailureDto> {
    state.plan_file_import_selection(request)
}

#[tauri::command]
pub async fn apply_file_import(
    state: State<'_, ImportApi>,
    request: ApplyFileImportRequestDto,
) -> Result<FileImportResultDto, CommandFailureDto> {
    state.apply_file_import(request)
}

#[tauri::command]
pub async fn apply_file_import_selection(
    state: State<'_, ImportApi>,
    request: ApplyFileImportSelectionRequestDto,
) -> Result<FileImportSelectionResultDto, CommandFailureDto> {
    state.apply_file_import_selection(request)
}

#[tauri::command]
pub fn cancel_file_import(
    state: State<'_, ImportApi>,
    request: CancelFileImportRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_file_import(request)
}

#[tauri::command]
pub async fn discover_git_import(
    state: State<'_, ImportApi>,
    request: DiscoverGitImportRequestDto,
) -> Result<GitImportDiscoveryDto, CommandFailureDto> {
    state.discover_git_import(request)
}

#[tauri::command]
pub async fn plan_git_import_selection(
    state: State<'_, ImportApi>,
    request: PlanGitImportSelectionRequestDto,
) -> Result<GitImportSelectionPreviewDto, CommandFailureDto> {
    state.plan_git_import_selection(request)
}

#[tauri::command]
pub async fn apply_git_import_selection(
    state: State<'_, ImportApi>,
    request: ApplyGitImportSelectionRequestDto,
) -> Result<GitImportSelectionResultDto, CommandFailureDto> {
    state.apply_git_import_selection(request)
}

#[tauri::command]
pub fn cancel_git_import_selection(
    state: State<'_, ImportApi>,
    request: CancelGitImportSelectionRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_git_import_selection(request)
}

#[tauri::command]
pub async fn check_skill_updates(
    state: State<'_, UpdateApi>,
    request: CheckSkillUpdatesRequestDto,
) -> Result<UpdateCheckReportDto, CommandFailureDto> {
    state.check_skill_updates(request)
}

#[tauri::command]
pub async fn plan_skill_updates(
    state: State<'_, UpdateApi>,
    request: PlanSkillUpdatesRequestDto,
) -> Result<UpdatePlanDto, CommandFailureDto> {
    state.plan_skill_updates(request)
}

#[tauri::command]
pub async fn apply_skill_updates(
    state: State<'_, UpdateApi>,
    request: ApplySkillUpdatesRequestDto,
) -> Result<UpdateResultDto, CommandFailureDto> {
    state.apply_skill_updates(request)
}

#[tauri::command]
pub async fn pin_skill_updates(
    state: State<'_, UpdateApi>,
    request: PinSkillUpdatesRequestDto,
) -> Result<(), CommandFailureDto> {
    state.pin_skill_updates(request)
}

#[tauri::command]
pub async fn scan_adopt(
    state: State<'_, AdoptApi>,
) -> Result<AdoptEvidenceReportDto, CommandFailureDto> {
    state.scan_adopt()
}

#[tauri::command]
pub async fn plan_adopt(
    state: State<'_, AdoptApi>,
    request: PlanAdoptRequestDto,
) -> Result<AdoptPlanDto, CommandFailureDto> {
    state.plan_adopt(request)
}

#[tauri::command]
pub async fn apply_adopt(
    app: AppHandle,
    state: State<'_, AdoptApi>,
    request: ApplyAdoptRequestDto,
) -> Result<AdoptResultDto, CommandFailureDto> {
    let result = state.apply_adopt(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub async fn undo_adopt(
    app: AppHandle,
    state: State<'_, AdoptApi>,
    request: UndoAdoptRequestDto,
) -> Result<AdoptUndoResultDto, CommandFailureDto> {
    let result = state.undo_adopt(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn finalize_adopt(
    state: State<'_, AdoptApi>,
    request: FinalizeAdoptRequestDto,
) -> Result<(), CommandFailureDto> {
    state.finalize_adopt(request)
}

#[tauri::command]
pub fn cancel_adopt(
    state: State<'_, AdoptApi>,
    request: CancelAdoptRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_adopt(request)
}

// -- Preferences & startup --

#[tauri::command]
pub fn load_preferences(
    state: State<'_, StartupApi>,
) -> Result<AppPreferencesDto, CommandFailureDto> {
    state.load_preferences()
}

#[tauri::command]
pub fn update_preferences(
    app: AppHandle,
    state: State<'_, StartupApi>,
    request: PreferenceUpdatesDto,
) -> Result<UpdatePreferencesResultDto, CommandFailureDto> {
    let result = state.update_preferences(request.clone())?;
    // Runtime side effects for the two preferences that touch the OS;
    // failures surface as a warning, the persisted value stays authoritative.
    let mut warning = None;
    if let Some(show_in_dock) = request.show_in_dock {
        if let Err(error) = lifecycle::apply_show_in_dock(&app, show_in_dock) {
            warning = Some(PreferencesWarningDto::ShowInDockFailed {
                detail: error.to_string(),
            });
        }
    }
    if let Some(launch_at_login) = request.launch_at_login {
        if let Err(error) = lifecycle::apply_launch_at_login(&app, launch_at_login) {
            warning = Some(PreferencesWarningDto::LaunchAtLoginFailed {
                detail: error.to_string(),
            });
        }
    }
    Ok(UpdatePreferencesResultDto {
        preferences: result.preferences,
        warning: warning.or(result.warning),
    })
}

#[tauri::command]
pub fn startup_info(state: State<'_, StartupApi>) -> Result<StartupInfoDto, CommandFailureDto> {
    state.startup_info()
}

#[tauri::command]
pub fn complete_onboarding(state: State<'_, StartupApi>) -> Result<(), CommandFailureDto> {
    state.complete_onboarding()
}

#[tauri::command]
pub fn create_agent_directory(
    state: State<'_, StartupApi>,
    request: CreateAgentDirectoryRequestDto,
) -> Result<StartupInfoDto, CommandFailureDto> {
    state.create_agent_directory(request)
}

// -- Fixture Recovery (spec §4.4, §5.2) --

#[tauri::command]
pub fn get_fixture_recovery_preview(
    state: State<'_, FixtureRecoveryApi>,
) -> Result<FixtureRecoveryPreviewDto, CommandFailureDto> {
    state.get_fixture_recovery_preview()
}

#[tauri::command]
pub fn plan_fixture_recovery(
    state: State<'_, FixtureRecoveryApi>,
    request: PlanFixtureRecoveryRequestDto,
) -> Result<FixtureRecoveryPlanDto, CommandFailureDto> {
    state.plan_fixture_recovery(request)
}

#[tauri::command]
pub fn restore_eligibility(
    state: State<'_, FixtureRecoveryApi>,
) -> Result<RestoreEligibilityDto, CommandFailureDto> {
    state.restore_eligibility()
}

#[tauri::command]
pub fn plan_restore(
    state: State<'_, FixtureRecoveryApi>,
) -> Result<FixtureRecoveryPlanDto, CommandFailureDto> {
    state.plan_restore()
}

#[tauri::command]
pub fn apply_fixture_recovery(
    state: State<'_, FixtureRecoveryApi>,
    request: ApplyFixtureRecoveryRequestDto,
) -> Result<RecoveryResultDto, CommandFailureDto> {
    state.apply_fixture_recovery(request)
}

#[tauri::command]
pub fn confirm_fixture_recovery_result(
    state: State<'_, FixtureRecoveryApi>,
    request: ConfirmFixtureRecoveryRequestDto,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    state.confirm_fixture_recovery_result(request)
}

#[tauri::command]
pub fn list_safety_snapshots(
    state: State<'_, FixtureRecoveryApi>,
) -> Result<Vec<SafetySnapshotDto>, CommandFailureDto> {
    state.list_safety_snapshots()
}

#[tauri::command]
pub fn plan_delete_safety_snapshot(
    state: State<'_, FixtureRecoveryApi>,
    request: DeleteSafetySnapshotRequestDto,
) -> Result<DeleteSnapshotPreviewDto, CommandFailureDto> {
    state.plan_delete_safety_snapshot(request)
}

#[tauri::command]
pub fn apply_delete_safety_snapshot(
    state: State<'_, FixtureRecoveryApi>,
    request: DeleteSafetySnapshotRequestDto,
) -> Result<(), CommandFailureDto> {
    state.apply_delete_safety_snapshot(request)
}
