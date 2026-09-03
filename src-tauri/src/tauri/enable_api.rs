//! Enable Module Tauri adapter (spec §4.9): the Global Target group query
//! and the plan → apply → undo/finalize command surface. All errors are
//! typed closed errors; diagnostics are raw and never user copy.

use crate::core::domain::SkillId;
use crate::core::enable::{CellResolution, EnableAction, EnableError, EnableService};
use crate::tauri_adapter::dto::{
    ApplyGlobalEnableRequestDto, CellResolutionDto, CellResolutionRequestDto, CommandFailureDto,
    DiagnosticDto, EnableActionDto, EnableOperationRequestDto, EnablePlanDto, EnableResultDto,
    EnableUndoResultDto, GlobalTargetGroupSnapshotDto, PlanGlobalEnableRequestDto,
    PlanGlobalLifecycleRequestDto, PublicErrorDto,
};

pub struct EnableApi {
    service: EnableService,
}

impl EnableApi {
    pub fn new(service: EnableService) -> Self {
        Self { service }
    }

    /// The current Skill's canonical Target groups (spec §4.9); nothing is
    /// created and a missing/mismatched Target only yields
    /// `open_agent_management`.
    pub fn list_target_groups(
        &self,
        skill_id: String,
    ) -> Result<GlobalTargetGroupSnapshotDto, CommandFailureDto> {
        self.service
            .list_target_groups(&SkillId(skill_id))
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn plan_global_enable(
        &self,
        request: PlanGlobalEnableRequestDto,
    ) -> Result<EnablePlanDto, CommandFailureDto> {
        self.service
            .plan_global_enable(
                &request
                    .skill_ids
                    .into_iter()
                    .map(SkillId)
                    .collect::<Vec<_>>(),
                &request.target_group_ids,
                &request
                    .cell_resolutions
                    .into_iter()
                    .map(
                        |CellResolutionRequestDto {
                             cell_key,
                             resolution,
                         }| { (cell_key, resolution.into()) },
                    )
                    .collect::<Vec<_>>(),
            )
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn plan_global_lifecycle(
        &self,
        request: PlanGlobalLifecycleRequestDto,
    ) -> Result<EnablePlanDto, CommandFailureDto> {
        self.service
            .plan_global_lifecycle(
                &SkillId(request.skill_id),
                &request.target_group_id,
                request.action.into(),
            )
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn apply_global_enable(
        &self,
        request: ApplyGlobalEnableRequestDto,
    ) -> Result<EnableResultDto, CommandFailureDto> {
        self.service
            .apply(&request.plan_token)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn undo_global_enable(
        &self,
        request: EnableOperationRequestDto,
    ) -> Result<EnableUndoResultDto, CommandFailureDto> {
        self.service
            .undo(&request.operation_id)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn finalize_global_enable(
        &self,
        request: EnableOperationRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .finalize(&request.operation_id)
            .map_err(command_error)
    }
}

impl From<CellResolutionDto> for CellResolution {
    fn from(value: CellResolutionDto) -> Self {
        match value {
            CellResolutionDto::Switch => Self::Switch,
            CellResolutionDto::Replace => Self::Replace,
            CellResolutionDto::Adopt => Self::Adopt,
            CellResolutionDto::Skip => Self::Skip,
        }
    }
}

impl From<EnableActionDto> for EnableAction {
    fn from(value: EnableActionDto) -> Self {
        match value {
            EnableActionDto::Enable => Self::Enable,
            EnableActionDto::Disable => Self::Disable,
            EnableActionDto::Repair => Self::Repair,
            EnableActionDto::Switch => Self::Switch,
        }
    }
}

fn command_error(error: EnableError) -> CommandFailureDto {
    let public_error = match &error {
        EnableError::Validation(_) => PublicErrorDto::Validation,
        EnableError::SkillNotFound { .. } => PublicErrorDto::NotFound,
        EnableError::PlanStale | EnableError::PlanNotFound => PublicErrorDto::PlanStale,
        EnableError::WriteGateClosed => PublicErrorDto::StateUnavailable,
        EnableError::SourceSnapshotMismatch => PublicErrorDto::SourceSnapshotMismatch,
        EnableError::TombstonedMember => PublicErrorDto::SourceSnapshotMismatch,
        EnableError::CellConflict(_) => PublicErrorDto::Validation,
        EnableError::RecoveryRequired(_) => PublicErrorDto::RecoveryRequired,
        EnableError::Store(
            crate::seams::activation_store::ActivationStoreError::EntryConflict { destination },
        ) => PublicErrorDto::Conflict {
            directory_name: destination.clone(),
        },
        EnableError::Store(_) => PublicErrorDto::StateUnavailable,
        EnableError::AgentStore(_) | EnableError::AgentFileSystem(_) => {
            PublicErrorDto::StateUnavailable
        }
        EnableError::Catalog(_) => PublicErrorDto::CatalogUnavailable,
        EnableError::FileSystem(crate::seams::filesystem::FileSystemError::RecoveryRequired {
            ..
        }) => PublicErrorDto::RecoveryRequired,
        EnableError::FileSystem(crate::seams::filesystem::FileSystemError::PlanStale {
            ..
        }) => PublicErrorDto::PlanStale,
        EnableError::FileSystem(crate::seams::filesystem::FileSystemError::Io {
            source, ..
        }) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            PublicErrorDto::PermissionDenied
        }
        EnableError::FileSystem(_) | EnableError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
