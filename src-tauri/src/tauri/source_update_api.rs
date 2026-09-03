use std::sync::Arc;

use crate::core::source_update::{SourceUpdateError, SourceUpdateService};
use crate::tauri_adapter::dto::{
    CommandFailureDto, DiagnosticDto, PreviewSourcePromotionRequestDto, PublicErrorDto,
    SourceUpdateConfirmRequestDto, SourceUpdateDraftDto,
};

pub struct SourceUpdateApi {
    service: Arc<SourceUpdateService>,
}

impl SourceUpdateApi {
    pub fn new(service: Arc<SourceUpdateService>) -> Self {
        Self { service }
    }

    /// Read-only classification of a fresh release against the managed
    /// source; the update write machinery is ticket #93.
    pub fn preview(
        &self,
        request: PreviewSourcePromotionRequestDto,
    ) -> Result<SourceUpdateDraftDto, CommandFailureDto> {
        self.service
            .preview(&request.remote_id, request.tracking_policy.map(Into::into))
            .map(Into::into)
            .map_err(command_error)
    }

    /// The Source Update confirmation stays closed until ticket #93.
    pub fn confirm(&self, request: SourceUpdateConfirmRequestDto) -> Result<(), CommandFailureDto> {
        self.service
            .confirm(&request.remote_id)
            .map_err(command_error)
    }
}

fn command_error(error: SourceUpdateError) -> CommandFailureDto {
    let public_error = match &error {
        SourceUpdateError::UpdateRestoredByTicket93 | SourceUpdateError::Validation(_) => {
            PublicErrorDto::Validation
        }
        SourceUpdateError::Preview(_) => PublicErrorDto::SourceUnavailable,
        SourceUpdateError::Store(_) => PublicErrorDto::CatalogUnavailable,
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
    fn the_closed_update_confirm_stays_typed_at_the_tauri_boundary() {
        let failure = command_error(SourceUpdateError::UpdateRestoredByTicket93);
        assert_eq!(failure.error, PublicErrorDto::Validation);
        assert_eq!(
            failure.diagnostic.expect("diagnostic").code,
            "source_update_failed"
        );
    }
}
