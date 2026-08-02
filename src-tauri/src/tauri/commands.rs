use tauri::State;

use crate::tauri_adapter::activation_api::ActivationApi;
use crate::tauri_adapter::catalog_api::CatalogApi;
use crate::tauri_adapter::dto::{
    ActivationHealthReportDto, ActivationPreviewDto, ActivationResultDto, AgentActivationDto,
    ApplyActivationRequestDto, CancelActivationRequestDto, CatalogListDto, CommandErrorDto,
    ListSkillsRequestDto, PlanActivationRepairRequestDto, PlanActivationRequestDto, SkillDetailDto,
};
use crate::tauri_adapter::health_api::HealthApi;

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
