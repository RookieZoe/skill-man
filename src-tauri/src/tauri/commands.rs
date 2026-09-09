use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::core::scan::mutation::ScanMutationCoordinator;
use crate::tauri_adapter::adopt_api::AdoptApi;
use crate::tauri_adapter::agent_configuration_api::AgentConfigurationApi;
use crate::tauri_adapter::app_update_api::AppUpdateApi;
use crate::tauri_adapter::bootstrap_api::BootstrapApi;
use crate::tauri_adapter::catalog_api::CatalogApi;
use crate::tauri_adapter::dto::{
    AbandonPreviewDto, ActivationHealthReportDto, AdoptPlanDto, AdoptResultDto, AdoptUndoResultDto,
    AgentConfigurationApplyResultDto, AgentConfigurationPlanDto, AgentManagementSnapshotDto,
    AppPreferencesDto, AppUpdateCheckDto, ApplyAbandonRequestDto, ApplyAdoptRequestDto,
    ApplyAgentConfigurationPlanRequestDto, ApplyFileImportRequestDto,
    ApplyFileImportSelectionRequestDto, ApplyFixtureRecoveryRequestDto,
    ApplyGlobalEnableRequestDto, ApplyLinkImportRequestDto, ApplyRelocateLinkRequestDto,
    ApplyRemoveSkillRequestDto, ApplySkillUpdatesRequestDto, BootstrapSnapshotDto,
    CancelAdoptRequestDto, CancelAppUpdateRequestDto, CancelExistingHomeRecoveryRequestDto,
    CancelFileImportRequestDto, CancelLinkImportRequestDto, CancelRelocateLinkRequestDto,
    CancelRemoveSkillRequestDto, CancelRescanRequestDto, CancelledAppUpdateDto,
    CandidateOperationRequestDto, CatalogListDto, CheckAppUpdateRequestDto,
    CheckSkillUpdatesRequestDto, CommandFailureDto, ConfirmExistingHomeRecoveryRequestDto,
    ConfirmFixtureRecoveryRequestDto, ConfirmHomeRequestDto, ConfirmSourcePromotionRequestDto,
    CreateAgentConfigurationRequestDto, CreateAgentDirectoryRequestDto,
    DeleteAgentConfigurationRequestDto, DeleteSafetySnapshotRequestDto, DeleteSnapshotPreviewDto,
    DiagnosticDto, DiscoverFileImportCollectionRequestDto, DiscoverFileImportRequestDto,
    DiscoverLinkImportRequestDto, DownloadAppUpdateRequestDto, DownloadedAppUpdateDto,
    EditAgentConfigurationRequestDto, EnableOperationRequestDto, EnablePlanDto, EnableResultDto,
    EnableUndoResultDto, ExistingHomeRecoveryPlanDto, FetchLatestAndManageRequestDto,
    FileImportCandidateDto, FileImportDiscoveryDto, FileImportPreviewDto, FileImportResultDto,
    FileImportSelectionPreviewDto, FileImportSelectionResultDto, FinalizeAdoptRequestDto,
    FixtureRecoveryPlanDto, FixtureRecoveryPreviewDto, GitSourceCapabilityReportDto,
    GlobalTargetGroupSnapshotDto, HomeCandidateDto, InstallAppUpdateRequestDto,
    LinkImportCandidateDto, LinkImportPreviewDto, LinkImportResultDto, ListSkillsRequestDto,
    LocaleSnapshotDto, ObservationAndScanSnapshotDto, PinSkillUpdatesRequestDto,
    PlanAdoptRequestDto, PlanFileImportRequestDto, PlanFileImportSelectionRequestDto,
    PlanFileReinstallRequestDto, PlanFixtureRecoveryRequestDto, PlanGlobalEnableRequestDto,
    PlanGlobalLifecycleRequestDto, PlanLinkImportRequestDto, PlanProjectEnableRequestDto,
    PlanRemoveSkillRequestDto, PlanSkillUpdatesRequestDto, PreferenceUpdatesDto,
    PreferencesWarningDto, PrepareExistingHomeRecoveryRequestDto, PrepareHomeRequestDto,
    PreviewSourcePromotionRequestDto, PublicErrorDto, RecentProjectFolderDto, RecoveryResultDto,
    RelocateLinkPreviewDto, RelocateLinkRequestDto, RelocateLinkResultDto, RemoveSkillPreviewDto,
    RemoveSkillResultDto, RestoreEligibilityDto, SafetySnapshotDto, SetLocaleSelectionRequestDto,
    SkillDetailDto, SourceGroupPreviewOutcomeDto, SourceLocalCopyRequestDto,
    SourceLocalCopyResultDto, SourcePromotionResultDto, SourceRemoveRequestDto,
    SourceRemoveResultDto, SourceRestoreRequestDto, SourceRestoreResultDto,
    SourceTransitionOperationRequestDto, SourceTransitionResultDto, SourceUndoResultDto,
    StartRescanRequestDto, StartupInfoDto, UndoAdoptRequestDto, UpdateCheckReportDto,
    UpdatePlanDto, UpdatePreferencesResultDto, UpdateResultDto,
};
use crate::tauri_adapter::enable_api::EnableApi;
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
use crate::tauri_adapter::source_update_api::SourceLifecycleApi;
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
pub async fn fetch_latest_and_manage(
    state: State<'_, SourceGroupPreviewApi>,
    request: FetchLatestAndManageRequestDto,
) -> Result<SourceGroupPreviewOutcomeDto, CommandFailureDto> {
    let api = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || api.fetch_latest_and_manage(request))
        .await
        .map_err(|_| CommandFailureDto {
            error: PublicErrorDto::Internal,
            diagnostic: None,
        })?
}

#[tauri::command]
pub async fn preview_source_promotion(
    state: State<'_, SourcePromotionApi>,
    request: PreviewSourcePromotionRequestDto,
) -> Result<crate::tauri_adapter::dto::SourcePromotionDraftOutcomeDto, CommandFailureDto> {
    let api = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || api.preview(request))
        .await
        .map_err(|_| CommandFailureDto {
            error: PublicErrorDto::Internal,
            diagnostic: None,
        })?
}

#[tauri::command]
pub async fn confirm_source_promotion(
    state: State<'_, SourcePromotionApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: ConfirmSourcePromotionRequestDto,
) -> Result<SourcePromotionResultDto, CommandFailureDto> {
    let api = state.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || api.confirm(request))
        .await
        .map_err(|_| CommandFailureDto {
            error: PublicErrorDto::Internal,
            diagnostic: None,
        })?;
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
pub async fn preview_source_update(
    state: State<'_, SourceUpdateApi>,
    request: PreviewSourcePromotionRequestDto,
) -> Result<crate::tauri_adapter::dto::SourceUpdateDraftDto, CommandFailureDto> {
    let api = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || api.preview(request))
        .await
        .map_err(|_| CommandFailureDto {
            error: PublicErrorDto::Internal,
            diagnostic: None,
        })?
}

#[tauri::command]
pub async fn confirm_source_update(
    state: State<'_, SourceUpdateApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: crate::tauri_adapter::dto::SourceUpdateConfirmRequestDto,
) -> Result<Option<SourcePromotionResultDto>, CommandFailureDto> {
    let api = state.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || api.confirm(request))
        .await
        .map_err(|_| CommandFailureDto {
            error: PublicErrorDto::Internal,
            diagnostic: None,
        })?;
    if matches!(&result, Ok(Some(_))) {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn undo_source_update(
    state: State<'_, SourceUpdateApi>,
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
pub fn restore_current_source_release(
    state: State<'_, SourceLifecycleApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: SourceRestoreRequestDto,
) -> Result<SourceRestoreResultDto, CommandFailureDto> {
    let result = state.restore_current_release(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn create_local_source_copy(
    state: State<'_, SourceLifecycleApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: SourceLocalCopyRequestDto,
) -> Result<SourceLocalCopyResultDto, CommandFailureDto> {
    let result = state.create_local_copy(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn remove_git_source(
    state: State<'_, SourceLifecycleApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: SourceRemoveRequestDto,
) -> Result<SourceRemoveResultDto, CommandFailureDto> {
    let result = state.remove_source(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub async fn confirm_source_transition(
    state: State<'_, SourceTransitionApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: crate::tauri_adapter::dto::ConfirmSourceTransitionRequestDto,
) -> Result<SourceTransitionResultDto, CommandFailureDto> {
    let api = state.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || api.confirm(request))
        .await
        .map_err(|_| CommandFailureDto {
            error: PublicErrorDto::Internal,
            diagnostic: None,
        })?;
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
pub fn get_observation_snapshot(
    state: State<'_, Arc<ObservationApi>>,
) -> ObservationAndScanSnapshotDto {
    state.snapshot()
}

#[tauri::command]
pub fn refresh_detection(state: State<'_, Arc<ObservationApi>>) -> ObservationAndScanSnapshotDto {
    state.refresh_detection()
}

#[tauri::command]
pub fn refresh_startup_probe(
    state: State<'_, Arc<ObservationApi>>,
) -> ObservationAndScanSnapshotDto {
    state.refresh_startup_probe()
}

#[tauri::command]
pub fn refresh_activation_health(
    state: State<'_, Arc<ObservationApi>>,
    request: crate::tauri_adapter::dto::RefreshActivationHealthRequestDto,
) -> ObservationAndScanSnapshotDto {
    state.refresh_activation_health(request.target_root_ids.as_deref())
}

#[tauri::command]
pub fn get_observation_page(
    state: State<'_, Arc<ObservationApi>>,
    request: crate::tauri_adapter::dto::ObservationPageRequestDto,
) -> Result<crate::tauri_adapter::dto::ObservationPageReadDto, CommandFailureDto> {
    state
        .observation_page(
            request.kind.into(),
            request.generation,
            crate::core::observation::ObservationCursor {
                offset: request.cursor.offset,
            },
            request.limit.unwrap_or(64),
        )
        .map_err(|error| observation_command_failure(&error))
}

fn observation_command_failure(
    error: &crate::core::observation::ObservationPageError,
) -> CommandFailureDto {
    let (public, diagnostic) = match error {
        crate::core::observation::ObservationPageError::Stale { current_generation } => (
            PublicErrorDto::ObservationPageStale {
                current_generation: *current_generation,
            },
            None,
        ),
        crate::core::observation::ObservationPageError::NotFound => {
            (PublicErrorDto::ObservationPageNotFound, None)
        }
    };
    CommandFailureDto {
        error: public,
        diagnostic: diagnostic.map(|message| DiagnosticDto {
            code: "observation_page".into(),
            message,
        }),
    }
}

#[tauri::command]
pub fn start_rescan(
    state: State<'_, Arc<ObservationApi>>,
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
    state: State<'_, Arc<ObservationApi>>,
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
        crate::core::scan::ScanError::ReportPageStale(current_generation) => (
            PublicErrorDto::ScanReportStale {
                current_generation: *current_generation,
            },
            None,
        ),
        crate::core::scan::ScanError::ReportPageNotFound => {
            (PublicErrorDto::ScanReportNotFound, None)
        }
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
pub fn get_scan_report_page(
    state: State<'_, Arc<ObservationApi>>,
    request: crate::tauri_adapter::dto::ScanReportPageRequestDto,
) -> Result<crate::tauri_adapter::dto::ScanReportPageDto, CommandFailureDto> {
    use crate::tauri_adapter::dto::scan_report_page_dto;
    let cursor = crate::seams::scan_evidence_store::ScanReportCursor {
        report_content_identity: request.cursor.report_content_identity,
        run_id: request.cursor.run_id,
        generation: request.cursor.generation,
        section: request.cursor.section.into(),
        offset: request.cursor.offset,
    };
    state
        .report_page(cursor.clone(), request.limit.unwrap_or(64) as usize)
        .map(|page| scan_report_page_dto(&cursor, page))
        .map_err(|error| scan_command_failure(&error))
}

#[tauri::command]
pub fn ignore_scan_local_candidate(
    state: State<'_, Arc<ObservationApi>>,
    report_content_identity: String,
    generation: u64,
    entity_seq: u64,
) -> Result<(), CommandFailureDto> {
    state
        .ignore_local_candidate(&report_content_identity, generation, entity_seq)
        .map_err(|error| scan_command_failure(&error))
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
    observation: State<'_, Arc<ObservationApi>>,
    request: ApplyAgentConfigurationPlanRequestDto,
) -> Result<AgentConfigurationApplyResultDto, CommandFailureDto> {
    let result = state.apply(request);
    if let Ok(apply_result) = &result {
        mutation.bump();
        // Spec §4.10 / ADR-0020: configuration Apply runs the Startup
        // Probe and schedules Activation health for exactly the affected
        // Targets (never a full Rescan, never all Targets).
        observation.refresh_startup_probe();
        let targets = apply_result.affected_target_root_ids.clone();
        let observation = observation.service_handle();
        std::thread::spawn(move || {
            observation.refresh_activation_health(Some(&targets));
        });
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
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: PinSkillUpdatesRequestDto,
) -> Result<(), CommandFailureDto> {
    let result = state.pin_skill_updates(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
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

// -- Global Enable (spec §4.9; ADR-0019) --

#[tauri::command]
pub fn list_target_groups(
    state: State<'_, EnableApi>,
    skill_id: String,
) -> Result<GlobalTargetGroupSnapshotDto, CommandFailureDto> {
    state.list_target_groups(skill_id)
}

#[tauri::command]
pub fn plan_global_enable(
    state: State<'_, EnableApi>,
    request: PlanGlobalEnableRequestDto,
) -> Result<EnablePlanDto, CommandFailureDto> {
    state.plan_global_enable(request)
}

#[tauri::command]
pub fn plan_global_lifecycle(
    state: State<'_, EnableApi>,
    request: PlanGlobalLifecycleRequestDto,
) -> Result<EnablePlanDto, CommandFailureDto> {
    state.plan_global_lifecycle(request)
}

#[tauri::command]
pub fn apply_global_enable(
    state: State<'_, EnableApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    observation: State<'_, Arc<ObservationApi>>,
    request: ApplyGlobalEnableRequestDto,
) -> Result<EnableResultDto, CommandFailureDto> {
    let result = state.apply_global_enable(request);
    if let Ok(result) = &result {
        mutation.bump();
        schedule_activation_health(
            &observation,
            result.cells.iter().map(|cell| cell.target_root_id.clone()),
        );
    }
    result
}

#[tauri::command]
pub fn undo_global_enable(
    state: State<'_, EnableApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    observation: State<'_, Arc<ObservationApi>>,
    request: EnableOperationRequestDto,
) -> Result<EnableUndoResultDto, CommandFailureDto> {
    let result = state.undo_global_enable(request);
    if let Ok(result) = &result {
        mutation.bump();
        schedule_activation_health(
            &observation,
            result.cells.iter().filter_map(|cell| {
                cell.cell_key
                    .split_once('|')
                    .map(|(_, target_root_id)| target_root_id.to_owned())
            }),
        );
    }
    result
}

#[tauri::command]
pub fn finalize_global_enable(
    state: State<'_, EnableApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: EnableOperationRequestDto,
) -> Result<(), CommandFailureDto> {
    let result = state.finalize_global_enable(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

/// Enable mutations commit desired Activation state in the Catalog. The
/// resulting Target groups must be re-observed so Inspector never keeps
/// showing the pre-mutation health. Deduplicate the physical Targets because
/// a batch can contain many cells for the same group.
fn schedule_activation_health<I>(observation: &Arc<ObservationApi>, target_root_ids: I)
where
    I: Iterator<Item = String>,
{
    let mut target_root_ids = target_root_ids.collect::<Vec<_>>();
    target_root_ids.sort();
    target_root_ids.dedup();
    if target_root_ids.is_empty() {
        return;
    }
    let observation = observation.service_handle();
    std::thread::spawn(move || {
        observation.refresh_activation_health(Some(&target_root_ids));
    });
}

// -- Project Enable (spec §4.9; ADR-0015; ADR-0019; #89) --

#[tauri::command]
pub fn list_recent_project_folders(
    state: State<'_, EnableApi>,
) -> Result<Vec<RecentProjectFolderDto>, CommandFailureDto> {
    state.list_recent_project_folders()
}

#[tauri::command]
pub fn clear_recent_project_folders(
    state: State<'_, EnableApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
) -> Result<(), CommandFailureDto> {
    let result = state.clear_recent_project_folders();
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn plan_project_enable(
    state: State<'_, EnableApi>,
    request: PlanProjectEnableRequestDto,
) -> Result<EnablePlanDto, CommandFailureDto> {
    state.plan_project_enable(request)
}

#[tauri::command]
pub fn apply_project_enable(
    state: State<'_, EnableApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: crate::tauri_adapter::dto::ApplyProjectEnableRequestDto,
) -> Result<EnableResultDto, CommandFailureDto> {
    let result = state.apply_project_enable(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn undo_project_enable(
    state: State<'_, EnableApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: EnableOperationRequestDto,
) -> Result<EnableUndoResultDto, CommandFailureDto> {
    let result = state.undo_project_enable(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub fn finalize_project_enable(
    state: State<'_, EnableApi>,
    mutation: State<'_, Arc<ScanMutationCoordinator>>,
    request: EnableOperationRequestDto,
) -> Result<(), CommandFailureDto> {
    let result = state.finalize_project_enable(request);
    if result.is_ok() {
        mutation.bump();
    }
    result
}

#[tauri::command]
pub async fn check_community_update(
    app: tauri::AppHandle,
) -> Result<Option<crate::core::community_update::CommunityUpdate>, String> {
    crate::adapters::community_release_source::check(&app.package_info().version.to_string()).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppAppearance {
    System,
    Light,
    Dark,
}

#[tauri::command]
pub fn set_app_appearance(app: tauri::AppHandle, appearance: AppAppearance) {
    app.set_theme(match appearance {
        AppAppearance::System => None,
        AppAppearance::Light => Some(tauri::Theme::Light),
        AppAppearance::Dark => Some(tauri::Theme::Dark),
    });
}
