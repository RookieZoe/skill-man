use std::sync::Arc;

use crate::core::source_promotion::{SourcePromotionError, SourcePromotionService};
use crate::tauri_adapter::dto::{
    CommandFailureDto, ConfirmSourcePromotionRequestDto, DiagnosticDto,
    PreviewSourcePromotionRequestDto, PublicErrorDto, SourcePromotionDraftOutcomeDto,
    SourcePromotionResultDto, SourceTransitionOperationRequestDto,
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
    ) -> Result<SourcePromotionDraftOutcomeDto, CommandFailureDto> {
        self.service
            .preview(&request.remote_id, request.tracking_policy.map(Into::into))
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
        SourcePromotionError::Validation(_) => PublicErrorDto::Validation,
        SourcePromotionError::Store(_) => PublicErrorDto::CatalogUnavailable,
        SourcePromotionError::Preview(_) => PublicErrorDto::SourceUnavailable,
        SourcePromotionError::Transition(_) => PublicErrorDto::StateUnavailable,
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
    fn an_unpromotable_legacy_state_stays_typed_at_the_tauri_boundary() {
        let failure = command_error(SourcePromotionError::Validation("bad legacy state".into()));
        assert_eq!(failure.error, PublicErrorDto::Validation);
        assert_eq!(
            failure.diagnostic.expect("diagnostic").code,
            "source_promotion_failed"
        );
    }
}
