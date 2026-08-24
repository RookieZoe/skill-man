//! Tauri translation for Existing Home Recovery Profile. The API exposes only
//! read-only preparation and in-memory cancellation; #67 owns confirmation
//! and the locator CAS.

use std::sync::Arc;

use crate::core::existing_home_recovery::{ExistingHomeRecoveryError, ExistingHomeRecoveryService};
use crate::tauri_adapter::dto::{
    CancelExistingHomeRecoveryRequestDto, CommandFailureDto, DiagnosticDto,
    ExistingHomeRecoveryPlanDto, PrepareExistingHomeRecoveryRequestDto, PublicErrorDto,
    RecoveryEligibilityRejectionDto, RecoveryProfileRejectionDto,
};

pub struct ExistingHomeRecoveryApi {
    service: Arc<ExistingHomeRecoveryService>,
}

impl ExistingHomeRecoveryApi {
    pub fn new(service: Arc<ExistingHomeRecoveryService>) -> Self {
        Self { service }
    }

    pub fn prepare(
        &self,
        request: PrepareExistingHomeRecoveryRequestDto,
    ) -> Result<ExistingHomeRecoveryPlanDto, CommandFailureDto> {
        self.service
            .prepare(std::path::Path::new(&request.path))
            .map(Into::into)
            .map_err(|error| failure(&error))
    }

    pub fn cancel(
        &self,
        request: CancelExistingHomeRecoveryRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .cancel(&request.plan_token)
            .map_err(|error| failure(&error))
    }
}

fn failure(error: &ExistingHomeRecoveryError) -> CommandFailureDto {
    let (public, diagnostic) = match error {
        ExistingHomeRecoveryError::Ineligible { reason } => (
            PublicErrorDto::ExistingHomeRecoveryIneligible {
                reason: (*reason).into(),
            },
            Some(DiagnosticDto {
                code: "existing_home_recovery_ineligible".into(),
                message: RecoveryEligibilityRejectionDto::from(*reason).code().into(),
            }),
        ),
        ExistingHomeRecoveryError::ProfileRejected { reason } => (
            PublicErrorDto::ExistingHomeRecoveryProfileRejected {
                reason: (*reason).into(),
            },
            Some(DiagnosticDto {
                code: "existing_home_recovery_profile_rejected".into(),
                message: RecoveryProfileRejectionDto::from(*reason).code().into(),
            }),
        ),
        ExistingHomeRecoveryError::PlanStale => (
            PublicErrorDto::PlanStale,
            Some(DiagnosticDto {
                code: "existing_home_recovery_plan_stale".into(),
                message: "plan_stale".into(),
            }),
        ),
        ExistingHomeRecoveryError::StateStore(_) => (
            PublicErrorDto::StateUnavailable,
            Some(DiagnosticDto {
                code: "existing_home_recovery_state_store".into(),
                message: "App-level state could not be read".into(),
            }),
        ),
        ExistingHomeRecoveryError::FileSystem(_) => (
            PublicErrorDto::RecoveryFilesystem,
            Some(DiagnosticDto {
                code: "existing_home_recovery_filesystem".into(),
                message: "selected Home could not be read".into(),
            }),
        ),
    };
    CommandFailureDto {
        error: public,
        diagnostic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::existing_home_recovery::RecoveryProfileRejection;

    #[test]
    fn profile_rejections_preserve_a_typed_reason_without_path_or_content() {
        let failure = failure(&ExistingHomeRecoveryError::ProfileRejected {
            reason: RecoveryProfileRejection::CatalogIntegrity,
        });
        assert!(matches!(
            failure.error,
            PublicErrorDto::ExistingHomeRecoveryProfileRejected { ref reason }
                if *reason == RecoveryProfileRejectionDto::CatalogIntegrity
        ));
        assert_eq!(
            failure.diagnostic.expect("diagnostic").message,
            "catalog_integrity"
        );
    }

    #[test]
    fn stale_plan_diagnostic_is_a_fixed_code_not_app_copy() {
        let failure = failure(&ExistingHomeRecoveryError::PlanStale);
        assert!(matches!(failure.error, PublicErrorDto::PlanStale));
        assert_eq!(
            failure.diagnostic.expect("diagnostic").message,
            "plan_stale"
        );
    }
}
