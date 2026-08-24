use std::sync::Arc;

use crate::core::source_update::{
    ConfirmSourceUpdateRequest, SourceUpdateError, SourceUpdateResult, SourceUpdateService,
};
use crate::tauri_adapter::dto::{
    CommandFailureDto, ConfirmSourcePromotionRequestDto, DiagnosticDto,
    PreviewSourcePromotionRequestDto, PublicErrorDto, SourcePromotionDraftDto,
    SourcePromotionResultDto, SourceTransitionOperationRequestDto,
};

pub struct SourceUpdateApi {
    service: Arc<SourceUpdateService>,
}

impl SourceUpdateApi {
    pub fn new(service: Arc<SourceUpdateService>) -> Self {
        Self { service }
    }

    pub fn preview(
        &self,
        request: PreviewSourcePromotionRequestDto,
    ) -> Result<SourcePromotionDraftDto, CommandFailureDto> {
        self.service
            .preview(&request.remote_id)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn confirm(
        &self,
        request: ConfirmSourcePromotionRequestDto,
    ) -> Result<SourcePromotionResultDto, CommandFailureDto> {
        self.service
            .confirm(ConfirmSourceUpdateRequest {
                remote_id: request.remote_id,
                expected_resolved_commit: request.expected_resolved_commit,
                resolutions: request.resolutions.into_iter().map(Into::into).collect(),
            })
            .map(result_dto)
            .map_err(command_error)
    }

    pub fn finalize(
        &self,
        request: SourceTransitionOperationRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .finalize(&request.operation_id)
            .map_err(command_error)
    }
}

fn result_dto(value: SourceUpdateResult) -> SourcePromotionResultDto {
    SourcePromotionResultDto {
        operation_id: value.operation_id,
        remote_id: value.remote_id,
        release_id: value.release_id,
        resolved_commit: value.resolved_commit,
        member_count: value.member_count,
        snapshot_version: value.snapshot_version,
        undo_available: false,
    }
}

fn command_error(error: SourceUpdateError) -> CommandFailureDto {
    let public_error = match &error {
        SourceUpdateError::PlanStale => PublicErrorDto::PlanStale,
        SourceUpdateError::OwnershipConflict | SourceUpdateError::Validation(_) => {
            PublicErrorDto::Validation
        }
        SourceUpdateError::Store(_) => PublicErrorDto::CatalogUnavailable,
        SourceUpdateError::RecoveryRequired(_) => PublicErrorDto::RecoveryRequired,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "source_update_failed".into(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_conflict_remains_typed_at_the_tauri_boundary() {
        let failure = command_error(SourceUpdateError::OwnershipConflict);
        assert_eq!(failure.error, PublicErrorDto::Validation);
        assert_eq!(
            failure.diagnostic.expect("diagnostic").code,
            "source_update_failed"
        );
    }

    #[test]
    fn plan_stale_remains_typed_at_the_tauri_boundary() {
        let failure = command_error(SourceUpdateError::PlanStale);
        assert_eq!(failure.error, PublicErrorDto::PlanStale);
    }
}
