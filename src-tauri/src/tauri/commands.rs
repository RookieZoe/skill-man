use tauri::{AppHandle, Emitter, State};

use crate::tauri_adapter::activation_api::ActivationApi;
use crate::tauri_adapter::adopt_api::AdoptApi;
use crate::tauri_adapter::app_update_api::AppUpdateApi;
use crate::tauri_adapter::catalog_api::CatalogApi;
use crate::tauri_adapter::dto::{
    ActivationConflictDetailsDto, ActivationConflictRequestDto, ActivationHealthReportDto,
    ActivationPreviewDto, ActivationReplacePreviewDto, ActivationReplaceUndoResultDto,
    ActivationResultDto, AdoptPlanDto, AdoptResultDto, AdoptScanReportDto, AdoptUndoResultDto,
    AgentActivationDto, AppPreferencesDto, AppUpdateCheckDto, ApplyActivationReplaceRequestDto,
    ApplyActivationRequestDto, ApplyAdoptRequestDto, ApplyFileImportRequestDto,
    ApplyFileImportSelectionRequestDto, ApplyGitImportSelectionRequestDto,
    ApplyLinkImportRequestDto, ApplyRelocateLinkRequestDto, ApplyRemoveSkillRequestDto,
    ApplySkillUpdatesRequestDto, CancelActivationReplaceRequestDto, CancelActivationRequestDto,
    CancelAdoptRequestDto, CancelAppUpdateRequestDto, CancelFileImportRequestDto,
    CancelGitImportSelectionRequestDto, CancelLinkImportRequestDto, CancelRelocateLinkRequestDto,
    CancelRemoveSkillRequestDto, CancelledAppUpdateDto, CatalogListDto, CheckAppUpdateRequestDto,
    CheckSkillUpdatesRequestDto, CommandErrorDto, CreateAgentDirectoryRequestDto,
    DiscoverFileImportCollectionRequestDto, DiscoverFileImportRequestDto,
    DiscoverGitImportRequestDto, DiscoverLinkImportRequestDto, DownloadAppUpdateRequestDto,
    DownloadedAppUpdateDto, FileImportCandidateDto, FileImportDiscoveryDto, FileImportPreviewDto,
    FileImportResultDto, FileImportSelectionPreviewDto, FileImportSelectionResultDto,
    FinalizeActivationReplaceRequestDto, FinalizeAdoptRequestDto, GitImportDiscoveryDto,
    GitImportSelectionPreviewDto, GitImportSelectionResultDto, InstallAppUpdateRequestDto,
    LinkImportCandidateDto, LinkImportPreviewDto, LinkImportResultDto, ListSkillsRequestDto,
    PinSkillUpdatesRequestDto, PlanActivationRepairRequestDto, PlanActivationReplaceRequestDto,
    PlanActivationRequestDto, PlanAdoptRequestDto, PlanFileImportRequestDto,
    PlanFileImportSelectionRequestDto, PlanFileReinstallRequestDto,
    PlanGitImportSelectionRequestDto, PlanLinkImportRequestDto, PlanRemoveSkillRequestDto,
    PlanSkillUpdatesRequestDto, PreferenceUpdatesDto, RelocateLinkPreviewDto,
    RelocateLinkRequestDto, RelocateLinkResultDto, RemoveSkillPreviewDto, RemoveSkillResultDto,
    SkillDetailDto, StartupInfoDto, UndoActivationReplaceRequestDto, UndoAdoptRequestDto,
    UpdateCheckReportDto, UpdatePlanDto, UpdatePreferencesResultDto, UpdateResultDto,
};
use crate::tauri_adapter::health_api::HealthApi;
use crate::tauri_adapter::import_api::ImportApi;
use crate::tauri_adapter::startup_api::StartupApi;
use crate::tauri_adapter::update_api::UpdateApi;
use crate::tauri_adapter::{lifecycle, tray};

#[tauri::command]
pub async fn check_app_update(
    state: State<'_, AppUpdateApi>,
    request: CheckAppUpdateRequestDto,
) -> Result<AppUpdateCheckDto, CommandErrorDto> {
    state.check_app_update(request).await
}

#[tauri::command]
pub async fn download_app_update(
    state: State<'_, AppUpdateApi>,
    request: DownloadAppUpdateRequestDto,
) -> Result<DownloadedAppUpdateDto, CommandErrorDto> {
    state.download_app_update(request).await
}

#[tauri::command]
pub fn cancel_app_update(
    state: State<'_, AppUpdateApi>,
    request: CancelAppUpdateRequestDto,
) -> Result<CancelledAppUpdateDto, CommandErrorDto> {
    state.cancel_app_update(request)
}

#[tauri::command]
pub fn install_app_update(
    state: State<'_, AppUpdateApi>,
    request: InstallAppUpdateRequestDto,
) -> Result<(), CommandErrorDto> {
    state.install_app_update(request)
}

#[tauri::command]
pub fn plan_activation(
    state: State<'_, ActivationApi>,
    request: PlanActivationRequestDto,
) -> Result<ActivationPreviewDto, CommandErrorDto> {
    state.plan_activation(request)
}

#[tauri::command]
pub fn plan_activation_repair(
    state: State<'_, ActivationApi>,
    request: PlanActivationRepairRequestDto,
) -> Result<ActivationPreviewDto, CommandErrorDto> {
    state.plan_activation_repair(request)
}

#[tauri::command]
pub async fn run_activation_health_check(
    app: AppHandle,
    state: State<'_, HealthApi>,
) -> Result<ActivationHealthReportDto, CommandErrorDto> {
    let result = state.run_activation_health_check()?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn relocate_link(
    state: State<'_, HealthApi>,
    request: RelocateLinkRequestDto,
) -> Result<RelocateLinkPreviewDto, CommandErrorDto> {
    state.relocate_link(request)
}

#[tauri::command]
pub fn apply_relocate_link(
    app: AppHandle,
    state: State<'_, HealthApi>,
    request: ApplyRelocateLinkRequestDto,
) -> Result<RelocateLinkResultDto, CommandErrorDto> {
    let result = state.apply_relocate_link(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn cancel_relocate_link(
    state: State<'_, HealthApi>,
    request: CancelRelocateLinkRequestDto,
) -> Result<bool, CommandErrorDto> {
    state.cancel_relocate_link(request)
}

#[tauri::command]
pub fn plan_remove_skill(
    state: State<'_, HealthApi>,
    request: PlanRemoveSkillRequestDto,
) -> Result<RemoveSkillPreviewDto, CommandErrorDto> {
    state.plan_remove_skill(request)
}

#[tauri::command]
pub fn apply_remove_skill(
    app: AppHandle,
    state: State<'_, HealthApi>,
    request: ApplyRemoveSkillRequestDto,
) -> Result<RemoveSkillResultDto, CommandErrorDto> {
    let result = state.apply_remove_skill(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn cancel_remove_skill(
    state: State<'_, HealthApi>,
    request: CancelRemoveSkillRequestDto,
) -> Result<bool, CommandErrorDto> {
    state.cancel_remove_skill(request)
}

#[tauri::command]
pub fn apply_activation(
    app: AppHandle,
    state: State<'_, ActivationApi>,
    request: ApplyActivationRequestDto,
) -> Result<ActivationResultDto, CommandErrorDto> {
    let result = state.apply_activation(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[::tauri::command]
pub fn cancel_activation(
    state: State<'_, ActivationApi>,
    request: CancelActivationRequestDto,
) -> Result<bool, CommandErrorDto> {
    state.cancel_activation(request)
}
#[tauri::command]
pub fn activation_conflict_details(
    state: State<'_, ActivationApi>,
    request: ActivationConflictRequestDto,
) -> Result<ActivationConflictDetailsDto, CommandErrorDto> {
    state.activation_conflict_details(request)
}

#[tauri::command]
pub fn plan_activation_replace(
    state: State<'_, ActivationApi>,
    request: PlanActivationReplaceRequestDto,
) -> Result<ActivationReplacePreviewDto, CommandErrorDto> {
    state.plan_activation_replace(request)
}

#[tauri::command]
pub fn apply_activation_replace(
    app: AppHandle,
    state: State<'_, ActivationApi>,
    request: ApplyActivationReplaceRequestDto,
) -> Result<ActivationResultDto, CommandErrorDto> {
    let result = state.apply_activation_replace(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn cancel_activation_replace(
    state: State<'_, ActivationApi>,
    request: CancelActivationReplaceRequestDto,
) -> Result<bool, CommandErrorDto> {
    state.cancel_activation_replace(request)
}

#[tauri::command]
pub fn undo_activation_replace(
    app: AppHandle,
    state: State<'_, ActivationApi>,
    request: UndoActivationReplaceRequestDto,
) -> Result<ActivationReplaceUndoResultDto, CommandErrorDto> {
    let result = state.undo_activation_replace(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn finalize_activation_replace(
    state: State<'_, ActivationApi>,
    request: FinalizeActivationReplaceRequestDto,
) -> Result<(), CommandErrorDto> {
    state.finalize_activation_replace(request)
}

#[tauri::command]
pub fn list_skills(
    state: State<'_, CatalogApi>,
    request: ListSkillsRequestDto,
) -> Result<CatalogListDto, CommandErrorDto> {
    state.list_skills(request)
}

#[tauri::command]
pub fn inspect_skill(
    state: State<'_, CatalogApi>,
    skill_id: String,
) -> Result<SkillDetailDto, CommandErrorDto> {
    state.inspect_skill(skill_id)
}

#[tauri::command]
pub fn list_agents(
    state: State<'_, CatalogApi>,
    skill_id: String,
) -> Result<Vec<AgentActivationDto>, CommandErrorDto> {
    state.list_agents(skill_id)
}

#[tauri::command]
pub fn discover_link_import(
    state: State<'_, ImportApi>,
    request: DiscoverLinkImportRequestDto,
) -> Result<LinkImportCandidateDto, CommandErrorDto> {
    state.discover_link_import(request)
}

#[tauri::command]
pub fn plan_link_import(
    state: State<'_, ImportApi>,
    request: PlanLinkImportRequestDto,
) -> Result<LinkImportPreviewDto, CommandErrorDto> {
    state.plan_link_import(request)
}

#[tauri::command]
pub fn apply_link_import(
    state: State<'_, ImportApi>,
    request: ApplyLinkImportRequestDto,
) -> Result<LinkImportResultDto, CommandErrorDto> {
    state.apply_link_import(request)
}

#[tauri::command]
pub fn cancel_link_import(
    state: State<'_, ImportApi>,
    request: CancelLinkImportRequestDto,
) -> Result<bool, CommandErrorDto> {
    state.cancel_link_import(request)
}

#[tauri::command]
pub fn discover_file_import(
    state: State<'_, ImportApi>,
    request: DiscoverFileImportRequestDto,
) -> Result<FileImportCandidateDto, CommandErrorDto> {
    state.discover_file_import(request)
}

#[tauri::command]
pub fn discover_file_import_collection(
    state: State<'_, ImportApi>,
    request: DiscoverFileImportCollectionRequestDto,
) -> Result<FileImportDiscoveryDto, CommandErrorDto> {
    state.discover_file_import_collection(request)
}

#[tauri::command]
pub fn plan_file_import(
    state: State<'_, ImportApi>,
    request: PlanFileImportRequestDto,
) -> Result<FileImportPreviewDto, CommandErrorDto> {
    state.plan_file_import(request)
}

#[tauri::command]
pub fn plan_file_reinstall(
    state: State<'_, ImportApi>,
    request: PlanFileReinstallRequestDto,
) -> Result<FileImportPreviewDto, CommandErrorDto> {
    state.plan_file_reinstall(request)
}

#[tauri::command]
pub fn plan_file_import_selection(
    state: State<'_, ImportApi>,
    request: PlanFileImportSelectionRequestDto,
) -> Result<FileImportSelectionPreviewDto, CommandErrorDto> {
    state.plan_file_import_selection(request)
}

#[tauri::command]
pub fn apply_file_import(
    state: State<'_, ImportApi>,
    request: ApplyFileImportRequestDto,
) -> Result<FileImportResultDto, CommandErrorDto> {
    state.apply_file_import(request)
}

#[tauri::command]
pub fn apply_file_import_selection(
    state: State<'_, ImportApi>,
    request: ApplyFileImportSelectionRequestDto,
) -> Result<FileImportSelectionResultDto, CommandErrorDto> {
    state.apply_file_import_selection(request)
}

#[tauri::command]
pub fn cancel_file_import(
    state: State<'_, ImportApi>,
    request: CancelFileImportRequestDto,
) -> Result<bool, CommandErrorDto> {
    state.cancel_file_import(request)
}

#[tauri::command]
pub fn discover_git_import(
    state: State<'_, ImportApi>,
    request: DiscoverGitImportRequestDto,
) -> Result<GitImportDiscoveryDto, CommandErrorDto> {
    state.discover_git_import(request)
}

#[tauri::command]
pub fn plan_git_import_selection(
    state: State<'_, ImportApi>,
    request: PlanGitImportSelectionRequestDto,
) -> Result<GitImportSelectionPreviewDto, CommandErrorDto> {
    state.plan_git_import_selection(request)
}

#[tauri::command]
pub fn apply_git_import_selection(
    state: State<'_, ImportApi>,
    request: ApplyGitImportSelectionRequestDto,
) -> Result<GitImportSelectionResultDto, CommandErrorDto> {
    state.apply_git_import_selection(request)
}

#[tauri::command]
pub fn cancel_git_import_selection(
    state: State<'_, ImportApi>,
    request: CancelGitImportSelectionRequestDto,
) -> Result<bool, CommandErrorDto> {
    state.cancel_git_import_selection(request)
}

#[tauri::command]
pub fn check_skill_updates(
    state: State<'_, UpdateApi>,
    request: CheckSkillUpdatesRequestDto,
) -> Result<UpdateCheckReportDto, CommandErrorDto> {
    state.check_skill_updates(request)
}

#[tauri::command]
pub fn plan_skill_updates(
    state: State<'_, UpdateApi>,
    request: PlanSkillUpdatesRequestDto,
) -> Result<UpdatePlanDto, CommandErrorDto> {
    state.plan_skill_updates(request)
}

#[tauri::command]
pub fn apply_skill_updates(
    state: State<'_, UpdateApi>,
    request: ApplySkillUpdatesRequestDto,
) -> Result<UpdateResultDto, CommandErrorDto> {
    state.apply_skill_updates(request)
}

#[tauri::command]
pub fn pin_skill_updates(
    state: State<'_, UpdateApi>,
    request: PinSkillUpdatesRequestDto,
) -> Result<(), CommandErrorDto> {
    state.pin_skill_updates(request)
}

#[tauri::command]
pub fn scan_adopt(state: State<'_, AdoptApi>) -> Result<AdoptScanReportDto, CommandErrorDto> {
    state.scan_adopt()
}

#[tauri::command]
pub fn plan_adopt(
    state: State<'_, AdoptApi>,
    request: PlanAdoptRequestDto,
) -> Result<AdoptPlanDto, CommandErrorDto> {
    state.plan_adopt(request)
}

#[tauri::command]
pub fn apply_adopt(
    app: AppHandle,
    state: State<'_, AdoptApi>,
    request: ApplyAdoptRequestDto,
) -> Result<AdoptResultDto, CommandErrorDto> {
    let result = state.apply_adopt(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn undo_adopt(
    app: AppHandle,
    state: State<'_, AdoptApi>,
    request: UndoAdoptRequestDto,
) -> Result<AdoptUndoResultDto, CommandErrorDto> {
    let result = state.undo_adopt(request)?;
    let _ = app.emit(tray::CATALOG_CHANGED_EVENT, ());
    Ok(result)
}

#[tauri::command]
pub fn finalize_adopt(
    state: State<'_, AdoptApi>,
    request: FinalizeAdoptRequestDto,
) -> Result<(), CommandErrorDto> {
    state.finalize_adopt(request)
}

#[tauri::command]
pub fn cancel_adopt(
    state: State<'_, AdoptApi>,
    request: CancelAdoptRequestDto,
) -> Result<bool, CommandErrorDto> {
    state.cancel_adopt(request)
}

// -- Preferences & startup --

#[tauri::command]
pub fn load_preferences(
    state: State<'_, StartupApi>,
) -> Result<AppPreferencesDto, CommandErrorDto> {
    state.load_preferences()
}

#[tauri::command]
pub fn update_preferences(
    app: AppHandle,
    state: State<'_, StartupApi>,
    request: PreferenceUpdatesDto,
) -> Result<UpdatePreferencesResultDto, CommandErrorDto> {
    let result = state.update_preferences(request.clone())?;
    // Runtime side effects for the two preferences that touch the OS;
    // failures surface as a warning, the persisted value stays authoritative.
    let mut warning = None;
    if let Some(show_in_dock) = request.show_in_dock {
        if let Err(error) = lifecycle::apply_show_in_dock(&app, show_in_dock) {
            warning = Some(format!("Dock 模式切换失败：{error}"));
        }
    }
    if let Some(launch_at_login) = request.launch_at_login {
        if let Err(error) = lifecycle::apply_launch_at_login(&app, launch_at_login) {
            warning = Some(format!("登录时启动设置失败：{error}"));
        }
    }
    Ok(UpdatePreferencesResultDto {
        preferences: result.preferences,
        warning: warning.or(result.warning),
    })
}

#[tauri::command]
pub fn startup_info(state: State<'_, StartupApi>) -> Result<StartupInfoDto, CommandErrorDto> {
    state.startup_info()
}

#[tauri::command]
pub fn complete_onboarding(state: State<'_, StartupApi>) -> Result<(), CommandErrorDto> {
    state.complete_onboarding()
}

#[tauri::command]
pub fn create_agent_directory(
    state: State<'_, StartupApi>,
    request: CreateAgentDirectoryRequestDto,
) -> Result<StartupInfoDto, CommandErrorDto> {
    state.create_agent_directory(request)
}
