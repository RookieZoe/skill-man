//! Enable Module Tauri adapter (spec §4.9): the Global Target group query
//! and the plan → apply → undo/finalize command surface. All errors are
//! typed closed errors; diagnostics are raw and never user copy.

use crate::core::domain::SkillId;
use crate::core::enable::{CellResolution, EnableAction, EnableError, EnableService};
use crate::tauri_adapter::dto::{
    ApplyGlobalEnableRequestDto, ApplyProjectEnableRequestDto, CellResolutionDto,
    CellResolutionRequestDto, CommandFailureDto, DiagnosticDto, EnableActionDto,
    EnableOperationRequestDto, EnablePlanDto, EnableResultDto, EnableUndoResultDto,
    GlobalTargetGroupSnapshotDto, PlanGlobalEnableRequestDto, PlanGlobalLifecycleRequestDto,
    PlanProjectEnableRequestDto, PublicErrorDto, RecentProjectFolderDto,
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

    pub fn list_recent_project_folders(
        &self,
    ) -> Result<Vec<RecentProjectFolderDto>, CommandFailureDto> {
        self.service
            .list_recent_project_folders()
            .map(|folders| folders.into_iter().map(Into::into).collect())
            .map_err(command_error)
    }

    pub fn remove_recent_project_folder(
        &self,
        canonical_path_key: &str,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .remove_recent_project_folder(canonical_path_key)
            .map_err(command_error)
    }

    pub fn clear_recent_project_folders(&self) -> Result<(), CommandFailureDto> {
        self.service
            .clear_recent_project_folders()
            .map_err(command_error)
    }

    pub fn plan_project_enable(
        &self,
        request: PlanProjectEnableRequestDto,
    ) -> Result<EnablePlanDto, CommandFailureDto> {
        self.service
            .plan_project_enable(
                &request
                    .skill_ids
                    .into_iter()
                    .map(SkillId)
                    .collect::<Vec<_>>(),
                std::path::Path::new(&request.project_folder),
                &request.agent_ids,
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

    pub fn apply_project_enable(
        &self,
        request: impl Into<ApplyProjectEnableRequestDto>,
    ) -> Result<EnableResultDto, CommandFailureDto> {
        let request = request.into();
        self.service
            .apply_project_confirmed(&request.plan_token, &request.confirmed_cell_keys)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn undo_project_enable(
        &self,
        request: EnableOperationRequestDto,
    ) -> Result<EnableUndoResultDto, CommandFailureDto> {
        self.service
            .undo(&request.operation_id)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn finalize_project_enable(
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
        EnableError::WriteGateClosed => PublicErrorDto::RecoveryRequired,
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
