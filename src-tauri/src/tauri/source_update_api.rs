//! Source Update / lifecycle Tauri boundary (ticket #93). The Core never
//! accepts a member list or release from the client: the API forwards only
//! the durable remote id, the frozen preview ref/commit and the tracking
//! override choice.

use std::sync::Arc;

use crate::core::source_lifecycle::{SourceLifecycleError, SourceLifecycleService};
use crate::core::source_update::{SourceUpdateError, SourceUpdateService};
use crate::tauri_adapter::dto::{
    CommandFailureDto, DiagnosticDto, PreviewSourcePromotionRequestDto, PublicErrorDto,
    SourceLocalCopyRequestDto, SourcePromotionResultDto, SourceRemoveRequestDto,
    SourceRemoveResultDto, SourceRestoreRequestDto, SourceRestoreResultDto,
    SourceTransitionOperationRequestDto, SourceUndoResultDto, SourceUpdateConfirmRequestDto,
    SourceUpdateDraftDto,
};

#[derive(Clone)]
pub struct SourceUpdateApi {
    update: Arc<SourceUpdateService>,
}

pub struct SourceLifecycleApi {
    lifecycle: Arc<SourceLifecycleService>,
}

impl SourceUpdateApi {
    pub fn new(update: Arc<SourceUpdateService>) -> Self {
        Self { update }
    }

    /// Read-only classification of a fresh release against the managed
    /// source (add/remove/reappear facts; the write stays in confirm).
    pub fn preview(
        &self,
        request: PreviewSourcePromotionRequestDto,
    ) -> Result<SourceUpdateDraftDto, CommandFailureDto> {
        self.update
            .preview(&request.remote_id, request.tracking_policy.map(Into::into))
            .map(Into::into)
            .map_err(update_error)
    }

    /// One whole-source Update through the immutable Source Transition.
    pub fn confirm(
        &self,
        request: SourceUpdateConfirmRequestDto,
    ) -> Result<SourcePromotionResultDto, CommandFailureDto> {
        self.update
            .confirm(
                &request.remote_id,
                None,
                request.expected_selected_ref,
                request.expected_resolved_commit,
            )
            .map(Into::into)
            .map_err(update_error)
    }

    pub fn undo(
        &self,
        request: SourceTransitionOperationRequestDto,
    ) -> Result<SourceUndoResultDto, CommandFailureDto> {
        self.update
            .undo(&request.operation_id)
            .map(Into::into)
            .map_err(update_error)
    }

    pub fn finalize(
        &self,
        request: SourceTransitionOperationRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.update
            .finalize(&request.operation_id)
            .map_err(update_error)
    }
}

impl SourceLifecycleApi {
    pub fn new(lifecycle: Arc<SourceLifecycleService>) -> Self {
        Self { lifecycle }
    }

    /// Restore Current Source Release: only the persisted current release
    /// bytes, journaled; never the latest.
    pub fn restore_current_release(
        &self,
        request: SourceRestoreRequestDto,
    ) -> Result<SourceRestoreResultDto, CommandFailureDto> {
        self.lifecycle
            .restore_current_release(&request.remote_id)
            .map(Into::into)
            .map_err(lifecycle_error)
    }

    /// Create Local Source Copy: observed member bytes to a user-chosen
    /// directory outside Home/Agent/installer roots.
    pub fn create_local_copy(
        &self,
        request: SourceLocalCopyRequestDto,
    ) -> Result<crate::tauri_adapter::dto::SourceLocalCopyResultDto, CommandFailureDto> {
        self.lifecycle
            .create_local_copy(
                &request.remote_id,
                &request.skill_id,
                std::path::Path::new(&request.destination),
            )
            .map(Into::into)
            .map_err(lifecycle_error)
    }

    /// Ordinary whole-source Remove (the source is the only Remove unit).
    pub fn remove_source(
        &self,
        request: SourceRemoveRequestDto,
    ) -> Result<SourceRemoveResultDto, CommandFailureDto> {
        self.lifecycle
            .remove_source(&request.remote_id)
            .map(Into::into)
            .map_err(lifecycle_error)
    }
}

fn update_error(error: SourceUpdateError) -> CommandFailureDto {
    match &error {
        SourceUpdateError::SourceSnapshotMismatch => {
            failure(PublicErrorDto::SourceSnapshotMismatch, &error)
        }
        SourceUpdateError::Validation(_) => failure(PublicErrorDto::Validation, &error),
        SourceUpdateError::RecoveryRequired(_) => failure(PublicErrorDto::RecoveryRequired, &error),
        SourceUpdateError::Preview(_) | SourceUpdateError::Source(_) => {
            failure(PublicErrorDto::SourceUnavailable, &error)
        }
        SourceUpdateError::Transition(
            crate::core::source_transition::SourceTransitionError::PreviewStale,
        ) => failure(PublicErrorDto::PlanStale, &error),
        SourceUpdateError::Transition(inner) => {
            crate::tauri_adapter::source_transition_api::transition_command_error(inner)
        }
        SourceUpdateError::Store(_) => failure(PublicErrorDto::StateUnavailable, &error),
        SourceUpdateError::FileSystem(
            crate::seams::filesystem::FileSystemError::RecoveryRequired { .. },
        ) => failure(PublicErrorDto::RecoveryRequired, &error),
        SourceUpdateError::FileSystem(_) => failure(PublicErrorDto::StateUnavailable, &error),
    }
}

fn lifecycle_error(error: SourceLifecycleError) -> CommandFailureDto {
    match &error {
        SourceLifecycleError::SourceSnapshotMismatch => {
            failure(PublicErrorDto::SourceSnapshotMismatch, &error)
        }
        SourceLifecycleError::Validation(_) => failure(PublicErrorDto::Validation, &error),
        SourceLifecycleError::RecoveryRequired(_) => {
            failure(PublicErrorDto::RecoveryRequired, &error)
        }
        SourceLifecycleError::Source(_) => failure(PublicErrorDto::SourceUnavailable, &error),
        SourceLifecycleError::Store(_) => failure(PublicErrorDto::StateUnavailable, &error),
        SourceLifecycleError::FileSystem(
            crate::seams::filesystem::FileSystemError::RecoveryRequired { .. },
        ) => failure(PublicErrorDto::RecoveryRequired, &error),
        SourceLifecycleError::FileSystem(_) => failure(PublicErrorDto::StateUnavailable, &error),
    }
}

fn failure(error: PublicErrorDto, source: &impl std::fmt::Display) -> CommandFailureDto {
    CommandFailureDto {
        error,
        diagnostic: Some(DiagnosticDto {
            code: "source_update_failed".into(),
            message: source.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mismatch_error_maps_to_a_typed_public_code() {
        let failure = update_error(SourceUpdateError::SourceSnapshotMismatch);
        assert_eq!(failure.error, PublicErrorDto::SourceSnapshotMismatch);
    }

    #[test]
    fn validation_errors_map_to_validation() {
        let failure = update_error(SourceUpdateError::Validation("no".into()));
        assert_eq!(failure.error, PublicErrorDto::Validation);
        let failure = lifecycle_error(SourceLifecycleError::RecoveryRequired("x".into()));
        assert_eq!(failure.error, PublicErrorDto::RecoveryRequired);
    }
}
