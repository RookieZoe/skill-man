//! Fixture Recovery Tauri API (spec §4.4, §5.2): inspect/plan/apply/confirm
//! plus Safety Snapshot listing and explicit deletion. Every command maps
//! Core errors to the closed `CommandFailureDto` shape — typed public codes
//! plus raw diagnostics, never free-form App Copy.

use std::sync::Arc;

use crate::adapters::runtime_catalog::RuntimeStoreSwitch;
use crate::core::bootstrap::BootstrapService;
use crate::core::fixture_recovery::{
    FixtureClassification, FixtureRecoveryError, FixtureRecoveryPreview, FixtureRecoverySelection,
    FixtureRecoveryService, RecoveryMode, RestoreEligibility, RestoreNotApplicableReason,
    RestoreReason,
};
use crate::seams::app_state_store::RecoveryOperationRecord;
use crate::tauri_adapter::bootstrap_api::BootstrapApi;
use crate::tauri_adapter::dto::{
    ActiveRecoveryOperationDto, ApplyFixtureRecoveryRequestDto, BootstrapSnapshotDto,
    CatalogEvidenceDto, CommandFailureDto, ConfirmFixtureRecoveryRequestDto,
    DeleteSafetySnapshotRequestDto, DeleteSnapshotPreviewDto, DiagnosticDto,
    FixtureClassificationDto, FixtureRecoveryPlanDto, FixtureRecoveryPreviewDto,
    PlanFixtureRecoveryRequestDto, PublicErrorDto, RecoveryModeDto, RecoveryResultDto,
    RestoreEligibilityDto, RestoreNotApplicableReasonDto, RestoreReasonDto, SafetySnapshotDto,
    TreeEvidenceDto,
};

pub struct FixtureRecoveryApi {
    service: Arc<FixtureRecoveryService>,
    bootstrap_service: Arc<BootstrapService>,
    store_switch: Arc<RuntimeStoreSwitch>,
    bootstrap: Arc<BootstrapApi>,
}

impl FixtureRecoveryApi {
    pub fn new(
        service: Arc<FixtureRecoveryService>,
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

    pub fn get_fixture_recovery_preview(
        &self,
    ) -> Result<FixtureRecoveryPreviewDto, CommandFailureDto> {
        self.service
            .inspect()
            .map(|preview| FixtureRecoveryPreviewDto::from(&preview))
            .map_err(|error| failure(&error))
    }

    pub fn plan_fixture_recovery(
        &self,
        _request: PlanFixtureRecoveryRequestDto,
    ) -> Result<FixtureRecoveryPlanDto, CommandFailureDto> {
        self.service
            .plan(&FixtureRecoverySelection {})
            .map(|plan| FixtureRecoveryPlanDto {
                plan_token: plan.plan_token,
            })
            .map_err(|error| failure(&error))
    }

    /// Restore eligibility probe (spec §5.5): closed reasons, never App
    /// Copy.
    pub fn restore_eligibility(&self) -> Result<RestoreEligibilityDto, CommandFailureDto> {
        self.service
            .restore_eligibility()
            .map(|eligibility| match eligibility {
                RestoreEligibility::RestoreRequired {
                    home_id,
                    path,
                    reason,
                } => RestoreEligibilityDto::RestoreRequired {
                    home_id: home_id.0,
                    path: path.to_string_lossy().into_owned(),
                    reason: match reason {
                        RestoreReason::CatalogIntegrityFailed => {
                            RestoreReasonDto::CatalogIntegrityFailed
                        }
                        RestoreReason::FixtureContamination => {
                            RestoreReasonDto::FixtureContamination
                        }
                    },
                },
                RestoreEligibility::NotRequired => RestoreEligibilityDto::NotRequired,
                RestoreEligibility::NotApplicable { reason } => {
                    RestoreEligibilityDto::NotApplicable {
                        reason: match reason {
                            RestoreNotApplicableReason::NoBinding => {
                                RestoreNotApplicableReasonDto::NoBinding
                            }
                            RestoreNotApplicableReason::LegacyUnbound => {
                                RestoreNotApplicableReasonDto::LegacyUnbound
                            }
                            RestoreNotApplicableReason::AppStateUnavailable => {
                                RestoreNotApplicableReasonDto::AppStateUnavailable
                            }
                            RestoreNotApplicableReason::IdentityMismatch => {
                                RestoreNotApplicableReasonDto::IdentityMismatch
                            }
                            RestoreNotApplicableReason::HomeUnavailable => {
                                RestoreNotApplicableReasonDto::HomeUnavailable
                            }
                            RestoreNotApplicableReason::UnsupportedSchema => {
                                RestoreNotApplicableReasonDto::UnsupportedSchema
                            }
                            RestoreNotApplicableReason::OpenFailed => {
                                RestoreNotApplicableReasonDto::OpenFailed
                            }
                            RestoreNotApplicableReason::ActiveOperation => {
                                RestoreNotApplicableReasonDto::ActiveOperation
                            }
                        },
                    }
                }
            })
            .map_err(|error| failure(&error))
    }

    /// User-initiated Restore of a same-identity content-failed Bound Home:
    /// opens the same crash-convergent operation (no pure-fixture gate).
    pub fn plan_restore(&self) -> Result<FixtureRecoveryPlanDto, CommandFailureDto> {
        self.service
            .plan_restore()
            .map(|plan| FixtureRecoveryPlanDto {
                plan_token: plan.plan_token,
            })
            .map_err(|error| failure(&error))
    }

    pub fn apply_fixture_recovery(
        &self,
        request: ApplyFixtureRecoveryRequestDto,
    ) -> Result<RecoveryResultDto, CommandFailureDto> {
        self.service
            .apply(&request.plan_token)
            .map(|result| RecoveryResultDto {
                operation_id: result.operation_id,
                awaiting_commit: result.awaiting_commit,
                rolled_back: result.rolled_back,
            })
            .map_err(|error| failure(&error))
    }

    /// Commit the recovery result and re-publish the bootstrap snapshot so
    /// the top-level route transitions out of the lock.
    pub fn confirm_fixture_recovery_result(
        &self,
        request: ConfirmFixtureRecoveryRequestDto,
    ) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
        let snapshot = self
            .service
            .confirm_result(&request.operation_id)
            .map_err(|error| failure(&error))?;
        // A Bound commit (fixture recovery or Restore) promoted a fresh
        // Home with the same identity: clear any stale open failure and
        // reopen the store facade so product writes work without rebuilding
        // the service graph.
        self.store_switch
            .reconcile_after_transition(&self.bootstrap_service, &snapshot)
            .map_err(|error| CommandFailureDto {
                error: PublicErrorDto::CatalogUnavailable,
                diagnostic: Some(DiagnosticDto {
                    code: "catalog_reopen_failed".into(),
                    message: error,
                }),
            })?;
        // Re-resolve through BootstrapApi so the write gate's active Home
        // context changes with the promoted Home before the Library mounts.
        self.bootstrap.get_bootstrap_snapshot()
    }

    pub fn list_safety_snapshots(&self) -> Result<Vec<SafetySnapshotDto>, CommandFailureDto> {
        self.service
            .list_snapshots()
            .map(|snapshots| {
                snapshots
                    .into_iter()
                    .map(|snapshot| SafetySnapshotDto {
                        snapshot_id: snapshot.snapshot_id,
                        path: snapshot.path.to_string_lossy().into_owned(),
                        manifest_hash: snapshot.manifest_hash,
                        file_count: snapshot.file_count,
                        total_bytes: snapshot.total_bytes,
                        taken_at: snapshot.taken_at,
                    })
                    .collect()
            })
            .map_err(|error| failure(&error))
    }

    pub fn plan_delete_safety_snapshot(
        &self,
        request: DeleteSafetySnapshotRequestDto,
    ) -> Result<DeleteSnapshotPreviewDto, CommandFailureDto> {
        self.service
            .plan_delete_snapshot(&request.plan_token)
            .map(|preview| DeleteSnapshotPreviewDto {
                snapshot_id: preview.snapshot_id,
                path: preview.path.to_string_lossy().into_owned(),
                file_count: preview.file_count,
                total_bytes: preview.total_bytes,
            })
            .map_err(|error| failure(&error))
    }

    pub fn apply_delete_safety_snapshot(
        &self,
        request: DeleteSafetySnapshotRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .apply_delete_snapshot(&request.plan_token)
            .map_err(|error| failure(&error))
    }
}

fn failure(error: &FixtureRecoveryError) -> CommandFailureDto {
    let (error, diagnostic) = match error {
        FixtureRecoveryError::SnapshotNotQualified(detail) => (
            PublicErrorDto::RecoverySnapshotNotQualified,
            Some(DiagnosticDto {
                code: "snapshot_not_qualified".into(),
                message: detail.clone(),
            }),
        ),
        FixtureRecoveryError::NotLocked => (PublicErrorDto::RecoveryNotLocked, None),
        FixtureRecoveryError::NotRestorable(message) => (
            PublicErrorDto::RestoreNotApplicable,
            Some(DiagnosticDto {
                code: "restore_not_applicable".into(),
                message: message.clone(),
            }),
        ),
        FixtureRecoveryError::NotPure => (PublicErrorDto::RecoveryNotPure, None),
        FixtureRecoveryError::NoActiveOperation => {
            (PublicErrorDto::RecoveryNoActiveOperation, None)
        }
        FixtureRecoveryError::OperationAlreadyActive { operation_id } => (
            PublicErrorDto::RecoveryOperationAlreadyActive,
            Some(DiagnosticDto {
                code: "operation_id".into(),
                message: operation_id.clone(),
            }),
        ),
        FixtureRecoveryError::WriterActive(message) => (
            PublicErrorDto::RecoveryWriterActive,
            Some(DiagnosticDto {
                code: "wal_lock".into(),
                message: message.clone(),
            }),
        ),
        FixtureRecoveryError::StepFailed {
            cursor,
            message,
            rolled_back,
        } => (
            PublicErrorDto::RecoveryStepFailed,
            Some(DiagnosticDto {
                code: format!("cursor_{cursor}_rolled_back_{rolled_back}"),
                message: message.clone(),
            }),
        ),
        FixtureRecoveryError::AmbiguousState(message) => (
            PublicErrorDto::RecoveryStateAmbiguous,
            Some(DiagnosticDto {
                code: "ambiguous".into(),
                message: message.clone(),
            }),
        ),
        FixtureRecoveryError::SnapshotInUse => (PublicErrorDto::RecoverySnapshotInUse, None),
        FixtureRecoveryError::StateStore(error) => (
            PublicErrorDto::RecoveryStateStore,
            Some(DiagnosticDto {
                code: "app_state".into(),
                message: error.to_string(),
            }),
        ),
        FixtureRecoveryError::FileSystem(message) => (
            PublicErrorDto::RecoveryFilesystem,
            Some(DiagnosticDto {
                code: "filesystem".into(),
                message: message.clone(),
            }),
        ),
        FixtureRecoveryError::Probe(message) => (
            PublicErrorDto::RecoveryProbe,
            Some(DiagnosticDto {
                code: "catalog_probe".into(),
                message: message.clone(),
            }),
        ),
    };
    CommandFailureDto { error, diagnostic }
}

impl From<&FixtureRecoveryPreview> for FixtureRecoveryPreviewDto {
    fn from(preview: &FixtureRecoveryPreview) -> Self {
        FixtureRecoveryPreviewDto {
            mode: match &preview.mode {
                RecoveryMode::LegacyUnbound => RecoveryModeDto::LegacyUnbound,
                RecoveryMode::BoundRestore { home_id } => RecoveryModeDto::BoundRestore {
                    home_id: home_id.0.clone(),
                },
            },
            path: preview.path.to_string_lossy().into_owned(),
            classification: match &preview.classification {
                FixtureClassification::Pure => FixtureClassificationDto::Pure,
                FixtureClassification::Mixed { reasons } => FixtureClassificationDto::Mixed {
                    reasons: reasons.clone(),
                },
                FixtureClassification::Unknown { reasons } => FixtureClassificationDto::Unknown {
                    reasons: reasons.clone(),
                },
                FixtureClassification::Clean => FixtureClassificationDto::Clean,
            },
            catalog_evidence: CatalogEvidenceDto {
                tables: preview.catalog_evidence.tables.clone(),
                schema_version: preview.catalog_evidence.schema_version,
                first_run_completed_at: preview.catalog_evidence.first_run_completed_at.clone(),
                skill_row_count: preview.catalog_evidence.skill_row_count,
                agent_row_count: preview.catalog_evidence.agent_row_count,
                activation_row_count: preview.catalog_evidence.activation_row_count,
                file_source_row_count: preview.catalog_evidence.file_source_row_count,
                remote_source_row_count: preview.catalog_evidence.remote_source_row_count,
            },
            tree_evidence: TreeEvidenceDto {
                fixture_entities_present: preview.tree_evidence.fixture_entities_present,
                skill_authoring_hash_matches: preview.tree_evidence.skill_authoring_hash_matches,
                media_xray_hash_matches: preview.tree_evidence.media_xray_hash_matches,
                root_hash_matches: preview.tree_evidence.root_hash_matches,
                legacy_audit_entity_present: preview.tree_evidence.legacy_audit_entity_present,
            },
            can_preview: preview.can_preview,
            active_operation: preview.active_operation.as_ref().map(active_operation_dto),
        }
    }
}

fn active_operation_dto(operation: &RecoveryOperationRecord) -> ActiveRecoveryOperationDto {
    ActiveRecoveryOperationDto {
        operation_id: operation.operation_id.clone(),
        cursor: operation.cursor.clone(),
        snapshot_path: operation
            .snapshot_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        prepared_path: operation
            .prepared_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_restore_mode_serializes_camel_case_fields() {
        // Spec §4.7: every public result uses camelCase fields. The tagged
        // enum must not leak snake_case variant fields to the client.
        let dto = FixtureRecoveryPreviewDto {
            mode: RecoveryModeDto::BoundRestore {
                home_id: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into(),
            },
            path: "/tmp/skill-man".into(),
            classification: FixtureClassificationDto::Pure,
            catalog_evidence: CatalogEvidenceDto {
                tables: vec![],
                schema_version: Some(4),
                first_run_completed_at: None,
                skill_row_count: 3,
                agent_row_count: 3,
                activation_row_count: 0,
                file_source_row_count: 0,
                remote_source_row_count: 0,
            },
            tree_evidence: TreeEvidenceDto {
                fixture_entities_present: true,
                skill_authoring_hash_matches: Some(true),
                media_xray_hash_matches: Some(true),
                root_hash_matches: Some(true),
                legacy_audit_entity_present: false,
            },
            can_preview: true,
            active_operation: None,
        };
        let json = serde_json::to_value(&dto).expect("serialize preview");
        assert_eq!(json["mode"]["kind"], "bound_restore");
        assert_eq!(
            json["mode"]["homeId"], "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            "variant fields must be camelCase on the wire"
        );
        assert_eq!(json["catalogEvidence"]["skillRowCount"], 3);
        assert_eq!(json["classification"]["kind"], "pure");
    }

    #[test]
    fn recovery_error_maps_to_closed_command_failure() {
        let failure = failure(&FixtureRecoveryError::WriterActive(
            "the SQLite WAL index is locked".into(),
        ));
        assert!(matches!(
            failure.error,
            super::PublicErrorDto::RecoveryWriterActive
        ));
        assert_eq!(
            failure
                .diagnostic
                .as_ref()
                .map(|diagnostic| diagnostic.code.as_str()),
            Some("wal_lock")
        );
    }

    #[test]
    fn restore_eligibility_serializes_typed_closed_reasons() {
        let required = RestoreEligibilityDto::RestoreRequired {
            home_id: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into(),
            path: "/tmp/skill-man".into(),
            reason: RestoreReasonDto::CatalogIntegrityFailed,
        };
        let json = serde_json::to_value(&required).expect("serialize");
        assert_eq!(json["kind"], "restore_required");
        assert_eq!(json["homeId"], "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab");
        assert_eq!(json["reason"], "catalog_integrity_failed");

        let not_applicable = RestoreEligibilityDto::NotApplicable {
            reason: RestoreNotApplicableReasonDto::IdentityMismatch,
        };
        let json = serde_json::to_value(&not_applicable).expect("serialize");
        assert_eq!(json["kind"], "not_applicable");
        assert_eq!(json["reason"], "identity_mismatch");

        let not_required = RestoreEligibilityDto::NotRequired;
        let json = serde_json::to_value(&not_required).expect("serialize");
        assert_eq!(json["kind"], "not_required");
    }
}
