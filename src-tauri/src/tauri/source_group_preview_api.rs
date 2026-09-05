use std::sync::Arc;

use crate::core::source_group_preview::{SourceGroupPreviewError, SourceGroupPreviewService};
use crate::tauri_adapter::dto::{
    CommandFailureDto, DiagnosticDto, FetchLatestAndManageRequestDto, PublicErrorDto,
    SourceGroupPreviewOutcomeDto,
};

#[derive(Clone)]
pub struct SourceGroupPreviewApi {
    service: Arc<SourceGroupPreviewService>,
}

impl SourceGroupPreviewApi {
    pub fn new(service: Arc<SourceGroupPreviewService>) -> Self {
        Self { service }
    }

    pub fn fetch_latest_and_manage(
        &self,
        request: FetchLatestAndManageRequestDto,
    ) -> Result<SourceGroupPreviewOutcomeDto, CommandFailureDto> {
        self.service
            .fetch_latest_and_manage(request.into())
            .map(Into::into)
            .map_err(command_error)
    }
}

fn command_error(error: SourceGroupPreviewError) -> CommandFailureDto {
    let public_error = match &error {
        SourceGroupPreviewError::Validation(_) => PublicErrorDto::Validation,
        SourceGroupPreviewError::Locks(_) => PublicErrorDto::StateUnavailable,
        SourceGroupPreviewError::Source(_) | SourceGroupPreviewError::Resolve(_) => {
            PublicErrorDto::SourceUnavailable
        }
        SourceGroupPreviewError::RemoteProvider(_) | SourceGroupPreviewError::TrackingPolicy(_) => {
            PublicErrorDto::SourceUnavailable
        }
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "source_group_preview_failed".into(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_stays_closed_at_the_tauri_boundary() {
        let failure = command_error(SourceGroupPreviewError::Validation("bad source".into()));
        assert_eq!(failure.error, PublicErrorDto::Validation);
        assert_eq!(
            failure.diagnostic.expect("diagnostic").code,
            "source_group_preview_failed"
        );
    }
}
