use crate::core::activation::{ActivationError, ActivationService, SetActivation};
use crate::core::domain::{AgentId, SkillId};
use crate::seams::filesystem::FileSystemError;
use crate::tauri_adapter::dto::{
    ActivationConflictDetailsDto, ActivationConflictRequestDto, ActivationPreviewDto,
    ActivationReplacePreviewDto, ActivationReplaceUndoResultDto, ActivationResultDto,
    ApplyActivationReplaceRequestDto, ApplyActivationRequestDto, CancelActivationReplaceRequestDto,
    CancelActivationRequestDto, CommandErrorDto, FinalizeActivationReplaceRequestDto,
    PlanActivationRepairRequestDto, PlanActivationReplaceRequestDto, PlanActivationRequestDto,
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
    ) -> Result<ActivationPreviewDto, CommandErrorDto> {
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
    ) -> Result<ActivationResultDto, CommandErrorDto> {
        self.activation
            .apply(&request.plan_token)
            .map(ActivationResultDto::from)
            .map_err(command_error)
    }

    pub fn plan_activation_repair(
        &self,
        request: PlanActivationRepairRequestDto,
    ) -> Result<ActivationPreviewDto, CommandErrorDto> {
        self.activation
            .plan_repair(SkillId(request.skill_id), AgentId(request.agent_id))
            .map(ActivationPreviewDto::from)
            .map_err(command_error)
    }

    pub fn cancel_activation(
        &self,
        request: CancelActivationRequestDto,
    ) -> Result<bool, CommandErrorDto> {
        self.activation
            .cancel(&request.plan_token)
            .map_err(command_error)
    }

    pub fn activation_conflict_details(
        &self,
        request: ActivationConflictRequestDto,
    ) -> Result<ActivationConflictDetailsDto, CommandErrorDto> {
        self.activation
            .conflict_details(&SkillId(request.skill_id), &AgentId(request.agent_id))
            .map(ActivationConflictDetailsDto::from)
            .map_err(command_error)
    }

    pub fn plan_activation_replace(
        &self,
        request: PlanActivationReplaceRequestDto,
    ) -> Result<ActivationReplacePreviewDto, CommandErrorDto> {
        self.activation
            .plan_replace(&SkillId(request.skill_id), &AgentId(request.agent_id))
            .map(ActivationReplacePreviewDto::from)
            .map_err(command_error)
    }

    pub fn apply_activation_replace(
        &self,
        request: ApplyActivationReplaceRequestDto,
    ) -> Result<ActivationResultDto, CommandErrorDto> {
        self.activation
            .apply_replace(&request.plan_token)
            .map(ActivationResultDto::from)
            .map_err(command_error)
    }

    pub fn cancel_activation_replace(
        &self,
        request: CancelActivationReplaceRequestDto,
    ) -> Result<bool, CommandErrorDto> {
        self.activation
            .cancel_replace(&request.plan_token)
            .map_err(command_error)
    }

    pub fn undo_activation_replace(
        &self,
        request: UndoActivationReplaceRequestDto,
    ) -> Result<ActivationReplaceUndoResultDto, CommandErrorDto> {
        self.activation
            .undo_replace(&request.operation_id)
            .map(ActivationReplaceUndoResultDto::from)
            .map_err(command_error)
    }

    pub fn finalize_activation_replace(
        &self,
        request: FinalizeActivationReplaceRequestDto,
    ) -> Result<(), CommandErrorDto> {
        self.activation
            .finalize_replace(&request.operation_id)
            .map_err(command_error)
    }
}

pub(crate) fn command_error(error: ActivationError) -> CommandErrorDto {
    let code = match &error {
        ActivationError::NotFound => "not_found",
        ActivationError::Validation(_)
        | ActivationError::AgentAdapter(_)
        | ActivationError::PathOverlap => "validation",
        ActivationError::Conflict(_) => "conflict",
        ActivationError::SourceUnavailable(_) => "source_unavailable",
        ActivationError::TargetMismatch(_) => "target_mismatch",
        ActivationError::PlanStale | ActivationError::PlanNotFound => "plan_stale",
        ActivationError::RecoveryRequired { .. } | ActivationError::RecoveryInProgress => {
            "recovery_required"
        }
        ActivationError::FileSystem(FileSystemError::RecoveryRequired { .. }) => {
            "recovery_required"
        }
        ActivationError::FileSystem(FileSystemError::PlanStale { .. }) => "plan_stale",
        ActivationError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            "permission_denied"
        }
        ActivationError::Store(_) => "state_unavailable",
        ActivationError::FileSystem(_) | ActivationError::Internal(_) => "internal",
    };
    CommandErrorDto {
        code: code.into(),
        message: error.to_string(),
    }
}
