//! Home Binding Tauri API (spec §4.2, §5.3–§5.4): prepare/confirm Home
//! candidates and continue/cancel interrupted binding operations. Every
//! command maps Core errors to the closed `CommandFailureDto` shape — typed
//! public codes plus raw diagnostics, never free-form App Copy.

use std::sync::Arc;

use crate::core::home_binding::{CandidateMode, HomeBindingError, HomeBindingService};
use crate::tauri_adapter::bootstrap_api::BootstrapApi;
use crate::tauri_adapter::dto::{
    BootstrapSnapshotDto, CandidateModeDto, CandidateOperationRequestDto, CommandFailureDto,
    ConfirmHomeRequestDto, DiagnosticDto, HomeCandidateDto, PrepareHomeRequestDto, PublicErrorDto,
};

pub struct HomeBindingApi {
    service: Arc<HomeBindingService>,
    bootstrap: Arc<BootstrapApi>,
}

impl HomeBindingApi {
    pub fn new(service: Arc<HomeBindingService>, bootstrap: Arc<BootstrapApi>) -> Self {
        Self { service, bootstrap }
    }

    /// Read-only candidate validation; returns the token `confirm_home`
    /// binds to this path and mode.
    pub fn prepare_home(
        &self,
        request: PrepareHomeRequestDto,
    ) -> Result<HomeCandidateDto, CommandFailureDto> {
        self.service
            .prepare_home(std::path::Path::new(&request.path))
            .map(HomeCandidateDto::from)
            .map_err(|error| failure(&error))
    }

    /// Explicit user confirmation: create/migrate/copy, verify, then commit
    /// the locator — the only binding commit point. Publishes
    /// `bootstrap://changed` with the fresh snapshot.
    pub fn confirm_home(
        &self,
        request: ConfirmHomeRequestDto,
    ) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
        let snapshot = self
            .service
            .confirm_home(&request.candidate_token)
            .map_err(|error| failure(&error))?;
        self.bootstrap.publish_changed();
        Ok(BootstrapSnapshotDto::from(&snapshot))
    }

    /// Resume an interrupted operation from its durable cursor.
    pub fn continue_candidate(
        &self,
        request: CandidateOperationRequestDto,
    ) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
        let snapshot = self
            .service
            .continue_candidate(&request.operation_id)
            .map_err(|error| failure(&error))?;
        self.bootstrap.publish_changed();
        Ok(BootstrapSnapshotDto::from(&snapshot))
    }

    /// Cancel an interrupted operation: delete only operation-created,
    /// identity-matched artifacts and clear the ledger.
    pub fn cancel_candidate(
        &self,
        request: CandidateOperationRequestDto,
    ) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
        let snapshot = self
            .service
            .cancel_candidate(&request.operation_id)
            .map_err(|error| failure(&error))?;
        self.bootstrap.publish_changed();
        Ok(BootstrapSnapshotDto::from(&snapshot))
    }
}

fn failure(error: &HomeBindingError) -> CommandFailureDto {
    let (public, diagnostic) = match error {
        HomeBindingError::InvalidState(message) => (
            PublicErrorDto::Validation,
            Some(DiagnosticDto {
                code: "binding_invalid_state".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::OperationAlreadyActive { operation_id } => (
            PublicErrorDto::RecoveryOperationAlreadyActive,
            Some(DiagnosticDto {
                code: "binding_operation_already_active".into(),
                message: operation_id.clone(),
            }),
        ),
        HomeBindingError::CandidateInvalid {
            reason,
            path,
            detail,
        } => (
            PublicErrorDto::CandidateInvalid {
                reason: reason_code(reason).into(),
            },
            Some(DiagnosticDto {
                code: "candidate_invalid".into(),
                message: format!("{}: {detail}", path.display()),
            }),
        ),
        HomeBindingError::NoActiveOperation => (
            PublicErrorDto::RecoveryNoActiveOperation,
            Some(DiagnosticDto {
                code: "binding_no_active_operation".into(),
                message: error.to_string(),
            }),
        ),
        HomeBindingError::OperationNotFound { operation_id } => (
            PublicErrorDto::NotFound,
            Some(DiagnosticDto {
                code: "binding_operation_not_found".into(),
                message: operation_id.clone(),
            }),
        ),
        HomeBindingError::WriterActive(message) => (
            PublicErrorDto::RecoveryWriterActive,
            Some(DiagnosticDto {
                code: "binding_writer_active".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::StepFailed { cursor, message } => (
            PublicErrorDto::BindingStepFailed {
                cursor: cursor.clone(),
            },
            Some(DiagnosticDto {
                code: "binding_step_failed".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::AmbiguousState(message) => (
            PublicErrorDto::BindingStateAmbiguous,
            Some(DiagnosticDto {
                code: "binding_state_ambiguous".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::StateStore(message) => (
            PublicErrorDto::RecoveryStateStore,
            Some(DiagnosticDto {
                code: "binding_state_store".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::Filesystem(message) => (
            PublicErrorDto::RecoveryFilesystem,
            Some(DiagnosticDto {
                code: "binding_filesystem".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::Probe(message) => (
            PublicErrorDto::RecoveryProbe,
            Some(DiagnosticDto {
                code: "binding_probe".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::Migration(message) => (
            PublicErrorDto::BindingMigrationFailed,
            Some(DiagnosticDto {
                code: "binding_migration".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::DiskFull {
            required_bytes,
            available_bytes,
        } => (
            PublicErrorDto::DiskFull {
                required_bytes: *required_bytes,
                available_bytes: *available_bytes,
            },
            Some(DiagnosticDto {
                code: "binding_insufficient_space".into(),
                message: error.to_string(),
            }),
        ),
        HomeBindingError::NotCancellable(message) => (
            PublicErrorDto::BindingNotCancellable,
            Some(DiagnosticDto {
                code: "binding_not_cancellable".into(),
                message: message.clone(),
            }),
        ),
        HomeBindingError::PlanStale => (
            PublicErrorDto::PlanStale,
            Some(DiagnosticDto {
                code: "binding_plan_stale".into(),
                message: "prepare the Home again".into(),
            }),
        ),
        HomeBindingError::Internal(message) => (
            PublicErrorDto::Internal,
            Some(DiagnosticDto {
                code: "binding_internal".into(),
                message: message.clone(),
            }),
        ),
    };
    CommandFailureDto {
        error: public,
        diagnostic,
    }
}

fn reason_code(reason: &crate::core::home_binding::CandidateInvalidReason) -> &'static str {
    use crate::core::home_binding::CandidateInvalidReason as R;
    match reason {
        R::NotAbsolute => "not_absolute",
        R::NotUtf8 => "not_utf8",
        R::SymlinkComponent => "symlink_component",
        R::StateDirOverlap => "state_dir_overlap",
        R::AgentDirOverlap => "agent_dir_overlap",
        R::ParentMissing => "parent_missing",
        R::ParentNotWritable => "parent_not_writable",
        R::NotDirectory => "not_directory",
        R::NotEmpty => "not_empty",
        R::NoVolumeIdentity => "no_volume_identity",
        R::InsufficientSpace => "insufficient_space",
        R::NotLegacyHome => "not_legacy_home",
        R::LegacyContaminated => "legacy_contaminated",
    }
}

impl From<crate::core::home_binding::HomeCandidate> for HomeCandidateDto {
    fn from(value: crate::core::home_binding::HomeCandidate) -> Self {
        Self {
            path: value.path.to_string_lossy().into_owned(),
            token: value.token,
            mode: match value.mode {
                CandidateMode::Fresh => CandidateModeDto::Fresh,
                CandidateMode::LegacyInPlace => CandidateModeDto::LegacyInPlace,
                CandidateMode::LegacyCopy => CandidateModeDto::LegacyCopy,
            },
            volume_fsid: value.volume.fsid,
            volume_uuid: value.volume.uuid,
            available_bytes: value.available_bytes,
            legacy_source: value
                .legacy_source
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::home_binding::CandidateInvalidReason;

    #[test]
    fn candidate_invalid_maps_to_typed_public_code() {
        let error = HomeBindingError::CandidateInvalid {
            reason: CandidateInvalidReason::SymlinkComponent,
            path: "/tmp/x".into(),
            detail: "path contains a symlink".into(),
        };
        let failure = failure(&error);
        match failure.error {
            PublicErrorDto::CandidateInvalid { reason } => {
                assert_eq!(reason, "symlink_component");
            }
            other => panic!("expected CandidateInvalid, got {other:?}"),
        }
        let diagnostic = failure.diagnostic.expect("diagnostic");
        assert_eq!(diagnostic.code, "candidate_invalid");
    }

    #[test]
    fn step_failure_maps_cursor_and_message() {
        let error = HomeBindingError::StepFailed {
            cursor: "verified".into(),
            message: "the marker is missing".into(),
        };
        let failure = failure(&error);
        match failure.error {
            PublicErrorDto::BindingStepFailed { cursor } => {
                assert_eq!(cursor, "verified");
            }
            other => panic!("expected BindingStepFailed, got {other:?}"),
        }
    }

    #[test]
    fn disk_full_carries_typed_bytes() {
        let error = HomeBindingError::DiskFull {
            required_bytes: 100,
            available_bytes: 42,
        };
        let failure = failure(&error);
        match failure.error {
            PublicErrorDto::DiskFull {
                required_bytes,
                available_bytes,
            } => {
                assert_eq!(required_bytes, 100);
                assert_eq!(available_bytes, 42);
            }
            other => panic!("expected DiskFull, got {other:?}"),
        }
    }
}
