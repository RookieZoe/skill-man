//! Tauri translation for Existing Home Recovery: read-only preparation,
//! cancellation and direct confirmation through the locator CAS.

use std::sync::Arc;

use crate::adapters::runtime_catalog::RuntimeStoreSwitch;
use crate::core::bootstrap::BootstrapService;
use crate::core::existing_home_recovery::{ExistingHomeRecoveryError, ExistingHomeRecoveryService};
use crate::tauri_adapter::bootstrap_api::BootstrapApi;
use crate::tauri_adapter::dto::{
    BootstrapSnapshotDto, CancelExistingHomeRecoveryRequestDto, CommandFailureDto,
    ConfirmExistingHomeRecoveryRequestDto, DiagnosticDto, ExistingHomeRecoveryPlanDto,
    PrepareExistingHomeRecoveryRequestDto, PublicErrorDto, RecoveryEligibilityRejectionDto,
    RecoveryProfileRejectionDto,
};

pub struct ExistingHomeRecoveryApi {
    service: Arc<ExistingHomeRecoveryService>,
    bootstrap_service: Arc<BootstrapService>,
    store_switch: Arc<RuntimeStoreSwitch>,
    bootstrap: Arc<BootstrapApi>,
}

impl ExistingHomeRecoveryApi {
    pub fn new(
        service: Arc<ExistingHomeRecoveryService>,
        bootstrap_service: Arc<BootstrapService>,
        store_switch: Arc<RuntimeStoreSwitch>,
        bootstrap: Arc<BootstrapApi>,
    ) -> Self {
        Self {
            service,
            bootstrap_service,
            store_switch,
            bootstrap,
        }
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

    /// A direct user confirmation: it revalidates the prepared facts, CASes
    /// only the locator, then reconciles the normal bootstrap/runtime state.
    /// There is deliberately no typed Home ID confirmation field.
    pub fn confirm(
        &self,
        request: ConfirmExistingHomeRecoveryRequestDto,
    ) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
        let snapshot = self
            .service
            .confirm(&request.plan_token)
            .map_err(|error| failure(&error))?;
        self.store_switch
            .reconcile_after_existing_home_recovery(&self.bootstrap_service, &snapshot)
            .map_err(|error| CommandFailureDto {
                error: PublicErrorDto::CatalogUnavailable,
                diagnostic: Some(DiagnosticDto {
                    code: "catalog_reconcile_failed".into(),
                    message: error,
                }),
            })?;
        self.bootstrap.get_bootstrap_snapshot()
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
        ExistingHomeRecoveryError::RecoveryInProgress => (
            PublicErrorDto::RecoveryRequired,
            Some(DiagnosticDto {
                code: "existing_home_recovery_in_progress".into(),
                message: error.to_string(),
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

    #[test]
    fn confirmation_dto_carries_only_the_opaque_reviewed_plan_token() {
        let request = ConfirmExistingHomeRecoveryRequestDto {
            plan_token: "ehr-opaque".into(),
        };
        assert_eq!(
            serde_json::to_value(request).expect("serialize confirmation request"),
            serde_json::json!({ "planToken": "ehr-opaque" })
        );
    }
}
