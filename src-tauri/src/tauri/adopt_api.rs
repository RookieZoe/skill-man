use std::path::PathBuf;

use crate::core::adopt::{AdoptError, AdoptSelection, AdoptService};
use crate::core::domain::AgentId;
use crate::tauri_adapter::dto::{
    AdoptPlanDto, AdoptResultDto, AdoptScanReportDto, AdoptUndoResultDto, ApplyAdoptRequestDto,
    CancelAdoptRequestDto, CommandFailureDto, DiagnosticDto, FinalizeAdoptRequestDto,
    PlanAdoptRequestDto, PublicErrorDto, UndoAdoptRequestDto,
};

pub struct AdoptApi {
    service: AdoptService,
}

impl AdoptApi {
    pub fn new(service: AdoptService) -> Self {
        Self { service }
    }

    pub fn scan_adopt(&self) -> Result<AdoptScanReportDto, CommandFailureDto> {
        self.service
            .scan()
            .map(|report| AdoptScanReportDto {
                candidates: report.candidates.into_iter().map(Into::into).collect(),
                truncated: report.truncated,
            })
            .map_err(command_error)
    }

    pub fn plan_adopt(
        &self,
        request: PlanAdoptRequestDto,
    ) -> Result<AdoptPlanDto, CommandFailureDto> {
        let selections = request
            .selections
            .into_iter()
            .map(|selection| AdoptSelection {
                canonical_entity: PathBuf::from(selection.canonical_entity),
                agent_ids: selection.agent_ids.into_iter().map(AgentId).collect(),
            })
            .collect::<Vec<_>>();
        let plan = self.service.plan(&selections).map_err(command_error)?;
        Ok(AdoptPlanDto {
            plan_token: plan.plan_token,
            items: plan
                .items
                .into_iter()
                .map(|item| crate::tauri_adapter::dto::AdoptPlanItemDto {
                    directory_name: item.directory_name,
                    canonical_entity: item.canonical_entity.to_string_lossy().into_owned(),
                    kind: match item.kind {
                        crate::core::adopt::AdoptPlanKind::Migrate => "migrate".into(),
                        crate::core::adopt::AdoptPlanKind::Link => "link".into(),
                    },
                    final_entity_path: item.final_entity_path.to_string_lossy().into_owned(),
                    appearances: item.appearances.into_iter().map(Into::into).collect(),
                    target_agents: item
                        .target_agents
                        .into_iter()
                        .map(|agent| crate::tauri_adapter::dto::AdoptTargetAgentDto {
                            agent_id: agent.agent_id.0,
                            name: agent.name,
                        })
                        .collect(),
                    adoptable: item.adoptable,
                    error: item.error,
                })
                .collect(),
            can_apply: plan.can_apply,
        })
    }

    pub fn apply_adopt(
        &self,
        request: ApplyAdoptRequestDto,
    ) -> Result<AdoptResultDto, CommandFailureDto> {
        let result = self
            .service
            .apply(&request.plan_token)
            .map_err(command_error)?;
        Ok(AdoptResultDto {
            operation_id: result.operation_id,
            items: result
                .items
                .into_iter()
                .map(|item| crate::tauri_adapter::dto::AdoptSkillResultDto {
                    skill_id: item.skill_id.0,
                    directory_name: item.directory_name,
                    adopted: item.adopted,
                    error: item.error,
                })
                .collect(),
            snapshot_version: result.snapshot_version,
            undo_available: result.undo_available,
        })
    }

    pub fn undo_adopt(
        &self,
        request: UndoAdoptRequestDto,
    ) -> Result<AdoptUndoResultDto, CommandFailureDto> {
        let result = self
            .service
            .undo(&request.operation_id)
            .map_err(command_error)?;
        Ok(AdoptUndoResultDto {
            operation_id: result.operation_id,
            items: result
                .items
                .into_iter()
                .map(|item| crate::tauri_adapter::dto::AdoptUndoItemResultDto {
                    directory_name: item.directory_name,
                    undone: item.undone,
                    error: item.error,
                })
                .collect(),
            snapshot_version: result.snapshot_version,
        })
    }

    pub fn finalize_adopt(
        &self,
        request: FinalizeAdoptRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .finalize(&request.operation_id)
            .map_err(command_error)
    }

    pub fn cancel_adopt(&self, request: CancelAdoptRequestDto) -> Result<bool, CommandFailureDto> {
        self.service
            .cancel(&request.plan_token)
            .map_err(command_error)
    }
}

fn command_error(error: AdoptError) -> CommandFailureDto {
    let public_error = match &error {
        AdoptError::Validation(_) => PublicErrorDto::Validation,
        AdoptError::PlanStale | AdoptError::PlanNotFound => PublicErrorDto::PlanStale,
        AdoptError::RecoveryRequired(_) => PublicErrorDto::RecoveryRequired,
        AdoptError::Store(crate::seams::adopt_store::AdoptStoreError::Conflict(directory_name)) => {
            PublicErrorDto::Conflict {
                directory_name: directory_name.clone(),
            }
        }
        AdoptError::Store(_) => PublicErrorDto::StateUnavailable,
        AdoptError::FileSystem(crate::seams::filesystem::FileSystemError::RecoveryRequired {
            ..
        }) => PublicErrorDto::RecoveryRequired,
        AdoptError::FileSystem(crate::seams::filesystem::FileSystemError::PlanStale { .. }) => {
            PublicErrorDto::PlanStale
        }
        AdoptError::FileSystem(crate::seams::filesystem::FileSystemError::Io {
            source, ..
        }) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            PublicErrorDto::PermissionDenied
        }
        AdoptError::FileSystem(_) | AdoptError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
