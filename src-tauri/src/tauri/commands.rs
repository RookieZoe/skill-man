use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::core::scan::mutation::ScanMutationCoordinator;
use crate::tauri_adapter::adopt_api::AdoptApi;
use crate::tauri_adapter::agent_configuration_api::AgentConfigurationApi;
use crate::tauri_adapter::app_update_api::AppUpdateApi;
use crate::tauri_adapter::bootstrap_api::BootstrapApi;
use crate::tauri_adapter::catalog_api::CatalogApi;
use crate::tauri_adapter::dto::{
    AbandonPreviewDto, ActivationHealthReportDto, AdoptEvidenceReportDto, AdoptPlanDto,
    AdoptResultDto, AdoptUndoResultDto, AgentConfigurationApplyResultDto,
    AgentConfigurationPlanDto, AgentManagementSnapshotDto, AppPreferencesDto, AppUpdateCheckDto,
    ApplyAbandonRequestDto, ApplyAdoptRequestDto, ApplyAgentConfigurationPlanRequestDto,
    ApplyFileImportRequestDto, ApplyFileImportSelectionRequestDto, ApplyFixtureRecoveryRequestDto,
    ApplyLinkImportRequestDto, ApplyRelocateLinkRequestDto, ApplyRemoveSkillRequestDto,
    ApplySkillUpdatesRequestDto, BootstrapSnapshotDto, CancelAdoptRequestDto,
    CancelAppUpdateRequestDto, CancelExistingHomeRecoveryRequestDto, CancelFileImportRequestDto,
    CancelLinkImportRequestDto, CancelRelocateLinkRequestDto, CancelRemoveSkillRequestDto,
    CancelRescanRequestDto, CancelledAppUpdateDto, CandidateOperationRequestDto, CatalogListDto,
    CheckAppUpdateRequestDto, CheckSkillUpdatesRequestDto, CommandFailureDto,
    ConfirmExistingHomeRecoveryRequestDto, ConfirmFixtureRecoveryRequestDto, ConfirmHomeRequestDto,
    ConfirmSourcePromotionRequestDto, CreateAgentConfigurationRequestDto,
    CreateAgentDirectoryRequestDto, DeleteAgentConfigurationRequestDto,
    DeleteSafetySnapshotRequestDto, DeleteSnapshotPreviewDto, DiagnosticDto,
    DiscoverFileImportCollectionRequestDto, DiscoverFileImportRequestDto,
    DiscoverLinkImportRequestDto, DownloadAppUpdateRequestDto, DownloadedAppUpdateDto,
    EditAgentConfigurationRequestDto, ExistingHomeRecoveryPlanDto, FetchLatestAndManageRequestDto,
    FileImportCandidateDto, FileImportDiscoveryDto, FileImportPreviewDto, FileImportResultDto,
    FileImportSelectionPreviewDto, FileImportSelectionResultDto, FinalizeAdoptRequestDto,
    FixtureRecoveryPlanDto, FixtureRecoveryPreviewDto, GitSourceCapabilityReportDto,
    HomeCandidateDto, InstallAppUpdateRequestDto, LinkImportCandidateDto, LinkImportPreviewDto,
    LinkImportResultDto, ListSkillsRequestDto, LocaleSnapshotDto, ObservationAndScanSnapshotDto,
    PinSkillUpdatesRequestDto, PlanAdoptRequestDto, PlanFileImportRequestDto,
    PlanFileImportSelectionRequestDto, PlanFileReinstallRequestDto, PlanFixtureRecoveryRequestDto,
    PlanLinkImportRequestDto, PlanRemoveSkillRequestDto, PlanSkillUpdatesRequestDto,
    PreferenceUpdatesDto, PreferencesWarningDto, PrepareExistingHomeRecoveryRequestDto,
    PrepareHomeRequestDto, PreviewSourcePromotionRequestDto, PublicErrorDto, RecoveryResultDto,
    RelocateLinkPreviewDto, RelocateLinkRequestDto, RelocateLinkResultDto, RemoveSkillPreviewDto,
    RemoveSkillResultDto, RestoreEligibilityDto, SafetySnapshotDto, SetLocaleSelectionRequestDto,
    SkillDetailDto, SourceGroupPreviewOutcomeDto, SourcePromotionDraftDto,
    SourcePromotionResultDto, SourcePromotionUndoResultDto, SourceTransitionOperationRequestDto,
    SourceTransitionResultDto, SourceUndoResultDto, StartRescanRequestDto, StartupInfoDto,
    UndoAdoptRequestDto, UpdateCheckReportDto, UpdatePlanDto, UpdatePreferencesResultDto,
    UpdateResultDto,
};
use crate::tauri_adapter::existing_home_recovery_api::ExistingHomeRecoveryApi;
use crate::tauri_adapter::fixture_recovery_api::FixtureRecoveryApi;
use crate::tauri_adapter::git_source_capability_api::GitSourceCapabilityApi;
use crate::tauri_adapter::health_api::HealthApi;
use crate::tauri_adapter::home_binding_api::HomeBindingApi;
use crate::tauri_adapter::home_lifecycle_api::HomeLifecycleApi;
use crate::tauri_adapter::import_api::ImportApi;
use crate::tauri_adapter::locale_api::LocaleApi;
use crate::tauri_adapter::observation_api::ObservationApi;
use crate::tauri_adapter::source_group_preview_api::SourceGroupPreviewApi;
use crate::tauri_adapter::source_promotion_api::SourcePromotionApi;
use crate::tauri_adapter::source_transition_api::SourceTransitionApi;
use crate::tauri_adapter::source_update_api::SourceUpdateApi;
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
pub fn prepare_existing_home_recovery(
    state: State<'_, ExistingHomeRecoveryApi>,
    request: PrepareExistingHomeRecoveryRequestDto,
) -> Result<ExistingHomeRecoveryPlanDto, CommandFailureDto> {
    state.prepare(request)
}

#[tauri::command]
pub fn cancel_existing_home_recovery(
    state: State<'_, ExistingHomeRecoveryApi>,
    request: CancelExistingHomeRecoveryRequestDto,
) -> Result<(), CommandFailureDto> {
    state.cancel(request)
}

#[tauri::command]
pub fn confirm_existing_home_recovery(
    state: State<'_, ExistingHomeRecoveryApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ConfirmExistingHomeRecoveryRequestDto,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    let result = state.confirm(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn confirm_home(
    state: State<'_, HomeBindingApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ConfirmHomeRequestDto,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    let result = state.confirm_home(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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
pub fn fetch_latest_and_manage(
    state: State<'_, SourceGroupPreviewApi>,
    request: FetchLatestAndManageRequestDto,
) -> Result<SourceGroupPreviewOutcomeDto, CommandFailureDto> {
    state.fetch_latest_and_manage(request)
}

#[tauri::command]
pub fn preview_source_promotion(
    state: State<'_, SourcePromotionApi>,
    request: PreviewSourcePromotionRequestDto,
) -> Result<SourcePromotionDraftDto, CommandFailureDto> {
    state.preview(request)
}

#[tauri::command]
pub fn confirm_source_promotion(
    state: State<'_, SourcePromotionApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ConfirmSourcePromotionRequestDto,
) -> Result<SourcePromotionResultDto, CommandFailureDto> {
    let result = state.confirm(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn undo_source_promotion(
    state: State<'_, SourcePromotionApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: SourceTransitionOperationRequestDto,
) -> Result<SourcePromotionUndoResultDto, CommandFailureDto> {
    let result = state.undo(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn finalize_source_promotion(
    state: State<'_, SourcePromotionApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: SourceTransitionOperationRequestDto,
) -> Result<(), CommandFailureDto> {
    let result = state.finalize(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn preview_source_update(
    state: State<'_, SourceUpdateApi>,
    request: PreviewSourcePromotionRequestDto,
) -> Result<SourcePromotionDraftDto, CommandFailureDto> {
    state.preview(request)
}

#[tauri::command]
pub fn confirm_source_update(
    state: State<'_, SourceUpdateApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ConfirmSourcePromotionRequestDto,
) -> Result<SourcePromotionResultDto, CommandFailureDto> {
    let result = state.confirm(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn finalize_source_update(
    state: State<'_, SourceUpdateApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: SourceTransitionOperationRequestDto,
) -> Result<(), CommandFailureDto> {
    let result = state.finalize(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn confirm_source_transition(
    state: State<'_, SourceTransitionApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: crate::tauri_adapter::dto::ConfirmSourceTransitionRequestDto,
) -> Result<SourceTransitionResultDto, CommandFailureDto> {
    let result = state.confirm(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn undo_source_transition(
    state: State<'_, SourceTransitionApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: SourceTransitionOperationRequestDto,
) -> Result<SourceUndoResultDto, CommandFailureDto> {
    let result = state.undo(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn finalize_source_transition(
    state: State<'_, SourceTransitionApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: SourceTransitionOperationRequestDto,
) -> Result<(), CommandFailureDto> {
    let result = state.finalize(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplyRelocateLinkRequestDto,
) -> Result<RelocateLinkResultDto, CommandFailureDto> {
    let result = state.apply_relocate_link(request)?;
    mutation.bump();
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplyRemoveSkillRequestDto,
) -> Result<RemoveSkillResultDto, CommandFailureDto> {
    let result = state.apply_remove_skill(request)?;
    mutation.bump();
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
pub fn get_agent_management_snapshot(
    state: State<'_, AgentConfigurationApi>,
) -> Result<AgentManagementSnapshotDto, CommandFailureDto> {
    state.snapshot()
}

#[tauri::command]
pub fn get_observation_snapshot(state: State<'_, ObservationApi>) -> ObservationAndScanSnapshotDto {
    state.snapshot()
}

#[tauri::command]
pub fn refresh_detection(state: State<'_, ObservationApi>) -> ObservationAndScanSnapshotDto {
    state.refresh_detection()
}

#[tauri::command]
pub fn start_rescan(
    state: State<'_, ObservationApi>,
    request: StartRescanRequestDto,
) -> Result<ObservationAndScanSnapshotDto, CommandFailureDto> {
    let trigger = match request.trigger.as_str() {
        "manual" => crate::core::scan::ScanTrigger::Manual,
        "onboarding" => crate::core::scan::ScanTrigger::Onboarding,
        _ => {
            return Err(CommandFailureDto {
                error: PublicErrorDto::Validation,
                diagnostic: None,
            });
        }
    };
    state
        .start_rescan(trigger)
        .map_err(|error| scan_command_failure(&error))
}

#[tauri::command]
pub fn cancel_rescan(
    state: State<'_, ObservationApi>,
    request: CancelRescanRequestDto,
) -> Result<ObservationAndScanSnapshotDto, CommandFailureDto> {
    state
        .cancel_rescan(&request.run_id)
        .map_err(|error| scan_command_failure(&error))
}

fn scan_command_failure(error: &crate::core::scan::ScanError) -> CommandFailureDto {
    let (public, diagnostic) = match error {
        crate::core::scan::ScanError::NotWritable(detail) => {
            (PublicErrorDto::ScanNotWritable, Some(detail.clone()))
        }
        crate::core::scan::ScanError::ConfigurationUnavailable(detail) => {
            (PublicErrorDto::CatalogUnavailable, Some(detail.clone()))
        }
        crate::core::scan::ScanError::StoreUnavailable(detail) => {
            (PublicErrorDto::CatalogUnavailable, Some(detail.clone()))
        }
        crate::core::scan::ScanError::RunNotFound(_) => (PublicErrorDto::ScanRunNotFound, None),
        crate::core::scan::ScanError::Superseded => (PublicErrorDto::PlanStale, None),
        crate::core::scan::ScanError::Internal(detail) => {
            (PublicErrorDto::Internal, Some(detail.clone()))
        }
    };
    CommandFailureDto {
        error: public,
        diagnostic: diagnostic.map(|message| DiagnosticDto {
            code: "scan".into(),
            message,
        }),
    }
}

#[tauri::command]
pub fn plan_create_agent_configuration(
    state: State<'_, AgentConfigurationApi>,
    request: CreateAgentConfigurationRequestDto,
) -> Result<AgentConfigurationPlanDto, CommandFailureDto> {
    state.plan_create(request)
}

#[tauri::command]
pub fn plan_edit_agent_configuration(
    state: State<'_, AgentConfigurationApi>,
    request: EditAgentConfigurationRequestDto,
) -> Result<AgentConfigurationPlanDto, CommandFailureDto> {
    state.plan_edit(request)
}

#[tauri::command]
pub fn plan_delete_agent_configuration(
    state: State<'_, AgentConfigurationApi>,
    request: DeleteAgentConfigurationRequestDto,
) -> Result<AgentConfigurationPlanDto, CommandFailureDto> {
    state.plan_delete(request)
}

#[tauri::command]
pub fn apply_agent_configuration_plan(
    state: State<'_, AgentConfigurationApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplyAgentConfigurationPlanRequestDto,
) -> Result<AgentConfigurationApplyResultDto, CommandFailureDto> {
    let result = state.apply(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplyLinkImportRequestDto,
) -> Result<LinkImportResultDto, CommandFailureDto> {
    let result = state.apply_link_import(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplyFileImportRequestDto,
) -> Result<FileImportResultDto, CommandFailureDto> {
    let result = state.apply_file_import(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub async fn apply_file_import_selection(
    state: State<'_, ImportApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplyFileImportSelectionRequestDto,
) -> Result<FileImportSelectionResultDto, CommandFailureDto> {
    let result = state.apply_file_import_selection(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn cancel_file_import(
    state: State<'_, ImportApi>,
    request: CancelFileImportRequestDto,
) -> Result<bool, CommandFailureDto> {
    state.cancel_file_import(request)
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplySkillUpdatesRequestDto,
) -> Result<UpdateResultDto, CommandFailureDto> {
    let result = state.apply_skill_updates(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplyAdoptRequestDto,
) -> Result<AdoptResultDto, CommandFailureDto> {
    let result = state.apply_adopt(request)?;
    mutation.bump();
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub async fn undo_adopt(
    app: AppHandle,
    state: State<'_, AdoptApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: UndoAdoptRequestDto,
) -> Result<AdoptUndoResultDto, CommandFailureDto> {
    let result = state.undo_adopt(request)?;
    mutation.bump();
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn finalize_adopt(
    state: State<'_, AdoptApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: FinalizeAdoptRequestDto,
) -> Result<(), CommandFailureDto> {
    let result = state.finalize_adopt(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: CreateAgentDirectoryRequestDto,
) -> Result<StartupInfoDto, CommandFailureDto> {
    let result = state.create_agent_directory(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ApplyFixtureRecoveryRequestDto,
) -> Result<RecoveryResultDto, CommandFailureDto> {
    let result = state.apply_fixture_recovery(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn confirm_fixture_recovery_result(
    state: State<'_, FixtureRecoveryApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ConfirmFixtureRecoveryRequestDto,
) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
    let result = state.confirm_fixture_recovery_result(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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
