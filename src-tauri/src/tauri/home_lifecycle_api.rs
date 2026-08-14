//! Home Lifecycle Tauri API (spec §5.5, §4.2; ADR-0012 §5–§6): Reconnect
//! Same Home and Abandon Home and Start New. Every command maps Core errors
//! to the closed `CommandFailureDto` shape and reconciles the shared store
//! facade with the fresh snapshot — Reconnect reopens the Catalog of the
//! restored identity, Abandon closes it. Restore eligibility and planning
//! live on the Fixture Recovery API (the same state machine executes them).

use std::sync::Arc;

use crate::adapters::runtime_catalog::RuntimeStoreSwitch;
use crate::core::bootstrap::BootstrapService;
use crate::core::home_lifecycle::{HomeLifecycleError, HomeLifecycleService};
use crate::tauri_adapter::bootstrap_api::BootstrapApi;
use crate::tauri_adapter::dto::{
    AbandonPreviewDto, ApplyAbandonRequestDto, BootstrapSnapshotDto, CommandFailureDto,
    DiagnosticDto, PublicErrorDto,
};

pub struct HomeLifecycleApi {
    service: Arc<HomeLifecycleService>,
    bootstrap_service: Arc<BootstrapService>,
    store_switch: Arc<RuntimeStoreSwitch>,
    bootstrap: Arc<BootstrapApi>,
}

impl HomeLifecycleApi {
    pub fn new(
        service: Arc<HomeLifecycleService>,
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

    /// Reconnect Same Home: re-runs the three-way verification. Success
    /// reopens the Catalog facade for the restored identity and re-aligns
    /// the write gate; failure returns the unchanged closed snapshot with
    /// zero locator/path side effects.
    pub fn reconnect_same_home(&self) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
        let snapshot = self
            .service
            .reconnect_same_home()
            .map_err(|error| failure(&error))?;
        // Re-verify with a fresh access decision (a stale writable-open
        // failure is cleared) and reopen the store facade for the restored
        // identity (ADR-0012 §6: 成功则重新打开 SQLite、放行写).
        self.store_switch
            .reconcile_after_transition(&self.bootstrap_service, &snapshot)
            .map_err(|error| CommandFailureDto {
                error: PublicErrorDto::CatalogUnavailable,
                diagnostic: Some(DiagnosticDto {
                    code: "catalog_reopen_failed".into(),
                    message: error,
                }),
            })?;
        // Aligns the write gate with the fresh snapshot and publishes
        // `bootstrap://changed` when the gate moved.
        self.bootstrap.get_bootstrap_snapshot()
    }

    /// Abandon preview: the high-friction facts the user must confirm.
    pub fn plan_abandon(&self) -> Result<AbandonPreviewDto, CommandFailureDto> {
        self.service
            .plan_abandon()
            .map(|preview| AbandonPreviewDto {
                home_id: preview.home_id.0,
                path: preview.path.to_string_lossy().into_owned(),
                bound_at: preview.bound_at,
                plan_token: preview.plan_token,
            })
            .map_err(|error| failure(&error))
    }

    /// Apply Abandon: the locator CAS is the only commit point. After the
    /// CAS the store facade closes (the old Home is no longer the app's
    /// Home) and the write gate re-aligns to the fresh Unconfigured /
    /// Abandoned route.
    pub fn apply_abandon(
        &self,
        request: &ApplyAbandonRequestDto,
    ) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
        let home_id = crate::core::home::HomeId(request.home_id.clone());
        if crate::core::home::HomeId::parse(&home_id.0).is_none() {
            return Err(failure(&HomeLifecycleError::ConfirmationMismatch));
        }
        let snapshot = self
            .service
            .apply_abandon(&request.plan_token, &home_id)
            .map_err(|error| failure(&error))?;
        self.store_switch
            .reconcile_after_transition(&self.bootstrap_service, &snapshot)
            .map_err(|error| CommandFailureDto {
                error: PublicErrorDto::CatalogUnavailable,
                diagnostic: Some(DiagnosticDto {
                    code: "catalog_close_failed".into(),
                    message: error,
                }),
            })?;
        self.bootstrap.get_bootstrap_snapshot()
    }
}

fn failure(error: &HomeLifecycleError) -> CommandFailureDto {
    let (public, diagnostic) = match error {
        HomeLifecycleError::ReconnectNotAvailable => (
            PublicErrorDto::ReconnectNotAvailable,
            Some(DiagnosticDto {
                code: "reconnect_not_available".into(),
                message: error.to_string(),
            }),
        ),
        HomeLifecycleError::NotAbandonable(message) => (
            PublicErrorDto::AbandonNotApplicable,
            Some(DiagnosticDto {
                code: "abandon_not_applicable".into(),
                message: message.clone(),
            }),
        ),
        HomeLifecycleError::ActiveOperation { operation_id } => (
            PublicErrorDto::RecoveryOperationAlreadyActive,
            Some(DiagnosticDto {
                code: "abandon_active_operation".into(),
                message: operation_id.clone(),
            }),
        ),
        HomeLifecycleError::ConfirmationMismatch => (
            PublicErrorDto::AbandonConfirmationMismatch,
            Some(DiagnosticDto {
                code: "abandon_confirmation_mismatch".into(),
                message: error.to_string(),
            }),
        ),
        HomeLifecycleError::CasConflict(message) => (
            PublicErrorDto::AbandonCasConflict,
            Some(DiagnosticDto {
                code: "abandon_cas_conflict".into(),
                message: message.clone(),
            }),
        ),
        HomeLifecycleError::PlanStale => (
            PublicErrorDto::PlanStale,
            Some(DiagnosticDto {
                code: "abandon_plan_stale".into(),
                message: "review the Abandon preview again".into(),
            }),
        ),
        HomeLifecycleError::StateStore(message) => (
            PublicErrorDto::RecoveryStateStore,
            Some(DiagnosticDto {
                code: "lifecycle_state_store".into(),
                message: message.clone(),
            }),
        ),
        HomeLifecycleError::Internal(message) => (
            PublicErrorDto::Internal,
            Some(DiagnosticDto {
                code: "lifecycle_internal".into(),
                message: message.clone(),
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
    use std::sync::Mutex;

    use crate::adapters::runtime_catalog::RuntimeCatalogStore;
    use crate::core::bootstrap::{BootstrapConfig, BootstrapService};
    use crate::core::fixture_recovery::FixedCleanClassifier;
    use crate::core::home::VolumeIdentity;
    use crate::core::write_gate::{ClosedReason, WriteGate, WriteGateState};
    use crate::seams::app_state_store::{
        AppStateFiles, AppStateStore, AppStateStoreError, HomeBindingFile, RecoveryLedgerFile,
    };
    use crate::seams::catalog_probe::{CatalogProbe, CatalogProbeError, CatalogProbeReport};
    use crate::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};
    use crate::tauri_adapter::bootstrap_api::BootstrapChangedEmitter;

    #[test]
    fn abandon_confirmation_mismatch_maps_to_a_typed_public_code() {
        let mapped = failure(&HomeLifecycleError::ConfirmationMismatch);
        assert!(matches!(
            mapped.error,
            PublicErrorDto::AbandonConfirmationMismatch
        ));
        let mapped = failure(&HomeLifecycleError::CasConflict("raced".into()));
        assert!(matches!(mapped.error, PublicErrorDto::AbandonCasConflict));
        let mapped = failure(&HomeLifecycleError::PlanStale);
        assert!(matches!(mapped.error, PublicErrorDto::PlanStale));
    }

    #[test]
    fn invalid_home_id_confirmation_is_rejected_before_the_service() {
        let api = HomeLifecycleApi::new(
            Arc::new(HomeLifecycleService::new(
                Arc::new(RejectingStore),
                Arc::new(dummy_bootstrap()),
            )),
            Arc::new(dummy_bootstrap()),
            Arc::new(RuntimeStoreSwitch::new(
                Arc::new(RuntimeCatalogStore::closed(Arc::new(
                    crate::adapters::macos_fs::MacOsFileSystem::new(std::path::PathBuf::from(
                        "/tmp",
                    )),
                ))),
                "skill-man.sqlite3".into(),
            )),
            Arc::new(BootstrapApi::new(
                Arc::new(dummy_bootstrap()),
                Arc::new(WriteGate::new(WriteGateState::Closed {
                    reason: ClosedReason::Unconfigured,
                })),
                Arc::new(NoopEmitter),
            )),
        );
        let result = api.apply_abandon(&ApplyAbandonRequestDto {
            plan_token: "ab-1".into(),
            home_id: "not-a-uuid".into(),
        });
        assert!(matches!(
            result,
            Err(CommandFailureDto {
                error: PublicErrorDto::AbandonConfirmationMismatch,
                ..
            })
        ));
    }

    struct RejectingStore;

    impl AppStateStore for RejectingStore {
        fn load(&self) -> Result<AppStateFiles, AppStateStoreError> {
            Err(AppStateStoreError::LocatorInvalid(
                "must not be reached".into(),
            ))
        }

        fn write_locator(&self, _binding: &HomeBindingFile) -> Result<(), AppStateStoreError> {
            Ok(())
        }

        fn write_recovery_ledger(
            &self,
            _ledger: &RecoveryLedgerFile,
        ) -> Result<(), AppStateStoreError> {
            Ok(())
        }
    }

    struct NoopEmitter;

    impl BootstrapChangedEmitter for NoopEmitter {
        fn emit_changed(&self, _payload: &crate::tauri_adapter::dto::BootstrapChangedPayloadDto) {}
    }

    struct FixedVolume(Option<VolumeIdentity>);

    impl VolumeIdentitySource for FixedVolume {
        fn volume_identity(
            &self,
            _path: &std::path::Path,
        ) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
            Ok(self.0.clone())
        }
    }

    struct FixedProbe(Mutex<CatalogProbeReport>);

    impl CatalogProbe for FixedProbe {
        fn probe(&self, _path: &std::path::Path) -> Result<CatalogProbeReport, CatalogProbeError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn dummy_bootstrap() -> BootstrapService {
        BootstrapService::new(
            Arc::new(RejectingStore),
            Arc::new(FixedVolume(None)),
            Arc::new(FixedProbe(Mutex::new(CatalogProbeReport::absent()))),
            Arc::new(crate::adapters::macos_fs::MacOsFileSystem::new(
                std::path::PathBuf::from("/tmp"),
            )),
            Arc::new(FixedCleanClassifier),
            BootstrapConfig {
                state_dir: std::path::PathBuf::from("/tmp/state"),
                default_home_path: std::path::PathBuf::from("/tmp/home"),
                catalog_file_name: "skill-man.sqlite3".into(),
            },
        )
    }
}
