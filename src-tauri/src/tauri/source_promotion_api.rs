use std::sync::Arc;

use crate::core::source_promotion::{SourcePromotionError, SourcePromotionService};
use crate::tauri_adapter::dto::{
    CommandFailureDto, ConfirmSourcePromotionRequestDto, DiagnosticDto,
    PreviewSourcePromotionRequestDto, PublicErrorDto, SourcePromotionDraftDto,
    SourcePromotionResultDto, SourcePromotionUndoResultDto, SourceTransitionOperationRequestDto,
};

pub struct SourcePromotionApi {
    service: Arc<SourcePromotionService>,
}

impl SourcePromotionApi {
    pub fn new(service: Arc<SourcePromotionService>) -> Self {
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
            .confirm(request.into())
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn undo(
        &self,
        request: SourceTransitionOperationRequestDto,
    ) -> Result<SourcePromotionUndoResultDto, CommandFailureDto> {
        self.service
            .undo(&request.operation_id)
            .map(Into::into)
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

fn command_error(error: SourcePromotionError) -> CommandFailureDto {
    let public_error = match &error {
        SourcePromotionError::Validation(_)
        | SourcePromotionError::Draft(_)
        | SourcePromotionError::OwnershipConflict => PublicErrorDto::Validation,
        SourcePromotionError::Store(_) => PublicErrorDto::CatalogUnavailable,
        SourcePromotionError::Preview(_) | SourcePromotionError::Source(_) => {
            PublicErrorDto::SourceUnavailable
        }
        SourcePromotionError::FileSystem(_) => PublicErrorDto::StateUnavailable,
        SourcePromotionError::RecoveryRequired(_) => PublicErrorDto::RecoveryRequired,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "source_promotion_failed".into(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_legacy_promotion_error_stays_typed_at_the_tauri_boundary() {
        let failure = command_error(SourcePromotionError::Validation("bad legacy state".into()));
        assert_eq!(failure.error, PublicErrorDto::Validation);
        assert_eq!(
            failure.diagnostic.expect("diagnostic").code,
            "source_promotion_failed"
        );
    }
}
