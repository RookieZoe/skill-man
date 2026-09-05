use std::sync::Arc;

use crate::core::source_transition::{SourceTransitionError, SourceTransitionService};
use crate::seams::installer_lock_store::LockReleaseError;
use crate::tauri_adapter::dto::{
    CommandFailureDto, ConfirmSourceTransitionRequestDto, DiagnosticDto, PublicErrorDto,
    SourceTransitionOperationRequestDto, SourceTransitionResultDto, SourceUndoResultDto,
};

#[derive(Clone)]
pub struct SourceTransitionApi {
    service: Arc<SourceTransitionService>,
}

impl SourceTransitionApi {
    pub fn new(service: Arc<SourceTransitionService>) -> Self {
        Self { service }
    }

    pub fn confirm(
        &self,
        request: ConfirmSourceTransitionRequestDto,
    ) -> Result<SourceTransitionResultDto, CommandFailureDto> {
        self.service
            .confirm(request.into())
            .map(Into::into)
            .map_err(|error| transition_command_error(&error))
    }

    pub fn undo(
        &self,
        request: SourceTransitionOperationRequestDto,
    ) -> Result<SourceUndoResultDto, CommandFailureDto> {
        self.service
            .undo(&request.operation_id)
            .map(Into::into)
            .map_err(|error| transition_command_error(&error))
    }

    pub fn finalize(
        &self,
        request: SourceTransitionOperationRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .finalize(&request.operation_id)
            .map_err(|error| transition_command_error(&error))
    }
}

pub(crate) fn transition_command_error(error: &SourceTransitionError) -> CommandFailureDto {
    let public_error = match error {
        SourceTransitionError::SourceSnapshotMismatch => PublicErrorDto::SourceSnapshotMismatch,
        SourceTransitionError::RecoveryRequired(_) => PublicErrorDto::RecoveryRequired,
        SourceTransitionError::ExternalOwnershipReappeared
        | SourceTransitionError::Validation(_) => PublicErrorDto::Validation,
        SourceTransitionError::PreviewStale => PublicErrorDto::PlanStale,
        SourceTransitionError::Preview(_) | SourceTransitionError::Source(_) => {
            PublicErrorDto::SourceUnavailable
        }
        SourceTransitionError::LockRelease(LockReleaseError::RecoveryRequired(_)) => {
            PublicErrorDto::RecoveryRequired
        }
        SourceTransitionError::Lock(_) | SourceTransitionError::LockRelease(_) => {
            PublicErrorDto::StateUnavailable
        }
        SourceTransitionError::FileSystem(crate::seams::filesystem::FileSystemError::Io {
            source,
            ..
        }) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            PublicErrorDto::PermissionDenied
        }
        SourceTransitionError::FileSystem(
            crate::seams::filesystem::FileSystemError::RecoveryRequired { .. },
        ) => PublicErrorDto::RecoveryRequired,
        SourceTransitionError::FileSystem(_)
        | SourceTransitionError::Store(_)
        | SourceTransitionError::PromotionStore(_)
        | SourceTransitionError::UpdateStore(_) => PublicErrorDto::StateUnavailable,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "source_transition_failed".into(),
            message: error.to_string(),
        }),
    }
}
