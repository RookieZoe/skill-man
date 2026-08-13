use crate::core::activation::{ActivationError, ActivationService, SetActivation};
use crate::core::domain::{AgentId, SkillId};
use crate::seams::filesystem::FileSystemError;
use crate::tauri_adapter::dto::{
    ActivationConflictDetailsDto, ActivationConflictRequestDto, ActivationPreviewDto,
    ActivationReplacePreviewDto, ActivationReplaceUndoResultDto, ActivationResultDto,
    ApplyActivationReplaceRequestDto, ApplyActivationRequestDto, CancelActivationReplaceRequestDto,
    CancelActivationRequestDto, CommandFailureDto, DiagnosticDto,
    FinalizeActivationReplaceRequestDto, PlanActivationRepairRequestDto,
    PlanActivationReplaceRequestDto, PlanActivationRequestDto, PublicErrorDto,
    UndoActivationReplaceRequestDto,
};

pub struct ActivationApi {
    activation: ActivationService,
}

impl ActivationApi {
    pub fn new(activation: ActivationService) -> Self {
        Self { activation }
    }

    pub fn plan_activation(
        &self,
        request: PlanActivationRequestDto,
    ) -> Result<ActivationPreviewDto, CommandFailureDto> {
        self.activation
            .plan(SetActivation {
                skill_id: SkillId(request.skill_id),
                agent_id: AgentId(request.agent_id),
                enabled: request.enabled,
            })
            .map(ActivationPreviewDto::from)
            .map_err(command_error)
    }

    pub fn apply_activation(
        &self,
        request: ApplyActivationRequestDto,
    ) -> Result<ActivationResultDto, CommandFailureDto> {
        self.activation
            .apply(&request.plan_token)
            .map(ActivationResultDto::from)
            .map_err(command_error)
    }

    pub fn plan_activation_repair(
        &self,
        request: PlanActivationRepairRequestDto,
    ) -> Result<ActivationPreviewDto, CommandFailureDto> {
        self.activation
            .plan_repair(SkillId(request.skill_id), AgentId(request.agent_id))
            .map(ActivationPreviewDto::from)
            .map_err(command_error)
    }

    pub fn cancel_activation(
        &self,
        request: CancelActivationRequestDto,
    ) -> Result<bool, CommandFailureDto> {
        self.activation
            .cancel(&request.plan_token)
            .map_err(command_error)
    }

    pub fn activation_conflict_details(
        &self,
        request: ActivationConflictRequestDto,
    ) -> Result<ActivationConflictDetailsDto, CommandFailureDto> {
        self.activation
            .conflict_details(&SkillId(request.skill_id), &AgentId(request.agent_id))
            .map(ActivationConflictDetailsDto::from)
            .map_err(command_error)
    }

    pub fn plan_activation_replace(
        &self,
        request: PlanActivationReplaceRequestDto,
    ) -> Result<ActivationReplacePreviewDto, CommandFailureDto> {
        self.activation
            .plan_replace(&SkillId(request.skill_id), &AgentId(request.agent_id))
            .map(ActivationReplacePreviewDto::from)
            .map_err(command_error)
    }

    pub fn apply_activation_replace(
        &self,
        request: ApplyActivationReplaceRequestDto,
    ) -> Result<ActivationResultDto, CommandFailureDto> {
        self.activation
            .apply_replace(&request.plan_token)
            .map(ActivationResultDto::from)
            .map_err(command_error)
    }

    pub fn cancel_activation_replace(
        &self,
        request: CancelActivationReplaceRequestDto,
    ) -> Result<bool, CommandFailureDto> {
        self.activation
            .cancel_replace(&request.plan_token)
            .map_err(command_error)
    }

    pub fn undo_activation_replace(
        &self,
        request: UndoActivationReplaceRequestDto,
    ) -> Result<ActivationReplaceUndoResultDto, CommandFailureDto> {
        self.activation
            .undo_replace(&request.operation_id)
            .map(ActivationReplaceUndoResultDto::from)
            .map_err(command_error)
    }

    pub fn finalize_activation_replace(
        &self,
        request: FinalizeActivationReplaceRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.activation
            .finalize_replace(&request.operation_id)
            .map_err(command_error)
    }
}

pub(crate) fn command_error(error: ActivationError) -> CommandFailureDto {
    let public_error = match &error {
        ActivationError::NotFound => PublicErrorDto::NotFound,
        ActivationError::Validation(_)
        | ActivationError::AgentAdapter(_)
        | ActivationError::PathOverlap => PublicErrorDto::Validation,
        ActivationError::Conflict(path) => PublicErrorDto::Conflict {
            directory_name: path.to_string_lossy().into_owned(),
        },
        ActivationError::SourceUnavailable(_) => PublicErrorDto::SourceUnavailable,
        ActivationError::TargetMismatch(_) => PublicErrorDto::TargetMismatch,
        ActivationError::PlanStale | ActivationError::PlanNotFound => PublicErrorDto::PlanStale,
        ActivationError::RecoveryRequired { .. } | ActivationError::RecoveryInProgress => {
            PublicErrorDto::RecoveryRequired
        }
        ActivationError::FileSystem(FileSystemError::RecoveryRequired { .. }) => {
            PublicErrorDto::RecoveryRequired
        }
        ActivationError::FileSystem(FileSystemError::PlanStale { .. }) => PublicErrorDto::PlanStale,
        ActivationError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            PublicErrorDto::PermissionDenied
        }
        ActivationError::Store(_) => PublicErrorDto::StateUnavailable,
        ActivationError::FileSystem(_) | ActivationError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
