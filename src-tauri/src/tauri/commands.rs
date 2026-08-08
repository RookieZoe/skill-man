use tauri::State;

use crate::tauri_adapter::activation_api::ActivationApi;
use crate::tauri_adapter::adopt_api::AdoptApi;
use crate::tauri_adapter::catalog_api::CatalogApi;
use crate::tauri_adapter::dto::{
    ActivationConflictDetailsDto, ActivationConflictRequestDto, ActivationHealthReportDto,
    ActivationPreviewDto, ActivationReplacePreviewDto, ActivationReplaceUndoResultDto,
    ActivationResultDto, AdoptPlanDto, AdoptResultDto, AdoptScanReportDto, AdoptUndoResultDto,
    AgentActivationDto, ApplyActivationReplaceRequestDto, ApplyActivationRequestDto,
    ApplyAdoptRequestDto, ApplyFileImportRequestDto, ApplyFileImportSelectionRequestDto,
    ApplyGitImportSelectionRequestDto, ApplyLinkImportRequestDto, ApplySkillUpdatesRequestDto,
    CancelActivationReplaceRequestDto, CancelActivationRequestDto, CancelAdoptRequestDto,
    CancelFileImportRequestDto, CancelGitImportSelectionRequestDto, CancelLinkImportRequestDto,
    CatalogListDto, CheckSkillUpdatesRequestDto, CommandErrorDto,
    DiscoverFileImportCollectionRequestDto, DiscoverFileImportRequestDto,
    DiscoverGitImportRequestDto, DiscoverLinkImportRequestDto, FileImportCandidateDto,
    FileImportDiscoveryDto, FileImportPreviewDto, FileImportResultDto,
    FileImportSelectionPreviewDto, FileImportSelectionResultDto,
    FinalizeActivationReplaceRequestDto, FinalizeAdoptRequestDto, GitImportDiscoveryDto,
    GitImportSelectionPreviewDto, GitImportSelectionResultDto, LinkImportCandidateDto,
    LinkImportPreviewDto, LinkImportResultDto, ListSkillsRequestDto, PinSkillUpdatesRequestDto,
    PlanActivationRepairRequestDto, PlanActivationReplaceRequestDto, PlanActivationRequestDto,
    PlanAdoptRequestDto, PlanFileImportRequestDto, PlanFileImportSelectionRequestDto,
    PlanFileReinstallRequestDto, PlanGitImportSelectionRequestDto, PlanLinkImportRequestDto,
    PlanSkillUpdatesRequestDto, SkillDetailDto, UndoActivationReplaceRequestDto,
    UndoAdoptRequestDto, UpdateCheckReportDto, UpdatePlanDto, UpdateResultDto,
};
use crate::tauri_adapter::health_api::HealthApi;
use crate::tauri_adapter::import_api::ImportApi;
use crate::tauri_adapter::update_api::UpdateApi;

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
    state: State<'_, HealthApi>,
) -> Result<ActivationHealthReportDto, CommandErrorDto> {
    state.run_activation_health_check()
}

#[tauri::command]
pub fn apply_activation(
    state: State<'_, ActivationApi>,
    request: ApplyActivationRequestDto,
) -> Result<ActivationResultDto, CommandErrorDto> {
    state.apply_activation(request)
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
    state: State<'_, ActivationApi>,
    request: ApplyActivationReplaceRequestDto,
) -> Result<ActivationResultDto, CommandErrorDto> {
    state.apply_activation_replace(request)
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
    state: State<'_, ActivationApi>,
    request: UndoActivationReplaceRequestDto,
) -> Result<ActivationReplaceUndoResultDto, CommandErrorDto> {
    state.undo_activation_replace(request)
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
    state: State<'_, AdoptApi>,
    request: ApplyAdoptRequestDto,
) -> Result<AdoptResultDto, CommandErrorDto> {
    state.apply_adopt(request)
}

#[tauri::command]
pub fn undo_adopt(
    state: State<'_, AdoptApi>,
    request: UndoAdoptRequestDto,
) -> Result<AdoptUndoResultDto, CommandErrorDto> {
    state.undo_adopt(request)
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
