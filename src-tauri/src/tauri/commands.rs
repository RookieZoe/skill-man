use tauri::State;

use crate::tauri_adapter::activation_api::ActivationApi;
use crate::tauri_adapter::catalog_api::CatalogApi;
use crate::tauri_adapter::dto::{
    ActivationHealthReportDto, ActivationPreviewDto, ActivationResultDto, AgentActivationDto,
    ApplyActivationRequestDto, ApplyLinkImportRequestDto, CancelActivationRequestDto,
    CancelLinkImportRequestDto, CatalogListDto, CommandErrorDto, DiscoverLinkImportRequestDto,
    LinkImportCandidateDto, LinkImportPreviewDto, LinkImportResultDto, ListSkillsRequestDto,
    PlanActivationRepairRequestDto, PlanActivationRequestDto, PlanLinkImportRequestDto,
    SkillDetailDto,
};
use crate::tauri_adapter::health_api::HealthApi;
use crate::tauri_adapter::import_api::ImportApi;

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
