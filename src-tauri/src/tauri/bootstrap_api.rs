//! Bootstrap Tauri API: `get_bootstrap_snapshot` plus the `bootstrap://changed`
//! event. The API reconciles the write gate with the current snapshot on
//! every query: a state change transitions the gate (bumping its generation
//! so outstanding plan tokens go stale) and emits the changed event.

use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::core::bootstrap::{
    BootstrapDiagnostic, BootstrapService, BootstrapSnapshot, CatalogAccess,
};
use crate::core::write_gate::{WriteGate, WriteGateState};
use crate::tauri_adapter::dto::{
    BootstrapChangedPayloadDto, BootstrapSnapshotDto, CatalogAccessDto, CatalogReadOnlyReasonDto,
    CommandFailureDto, DiagnosticDto, PublicErrorDto,
};

pub const BOOTSTRAP_CHANGED_EVENT: &str = "bootstrap://changed";

/// Emitter seam so API tests capture payloads without a Tauri runtime.
pub trait BootstrapChangedEmitter: Send + Sync {
    fn emit_changed(&self, payload: &BootstrapChangedPayloadDto);
}

pub struct TauriBootstrapChangedEmitter {
    app: AppHandle,
}

impl TauriBootstrapChangedEmitter {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl BootstrapChangedEmitter for TauriBootstrapChangedEmitter {
    fn emit_changed(&self, payload: &BootstrapChangedPayloadDto) {
        let _ = self.app.emit(BOOTSTRAP_CHANGED_EVENT, payload);
    }
}

pub struct BootstrapApi {
    service: Arc<BootstrapService>,
    write_gate: Arc<WriteGate>,
    emitter: Arc<dyn BootstrapChangedEmitter>,
}

impl BootstrapApi {
    pub fn new(
        service: Arc<BootstrapService>,
        write_gate: Arc<WriteGate>,
        emitter: Arc<dyn BootstrapChangedEmitter>,
    ) -> Self {
        Self {
            service,
            write_gate,
            emitter,
        }
    }

    pub fn get_bootstrap_snapshot(&self) -> Result<BootstrapSnapshotDto, CommandFailureDto> {
        let snapshot = self.service.inspect();
        let bound_home = self.service.verified_bound_home();
        let desired = snapshot.write_gate_state(bound_home.as_ref());
        let current = self.write_gate.snapshot();
        // A Recovery state is operation-owned (startup recovery in progress):
        // the recovery pass, not a snapshot query, reopens product writes.
        let recovery_owned = matches!(current.state, WriteGateState::Recovery { .. });
        if !recovery_owned && current.state != desired {
            self.write_gate
                .transition_to(desired)
                .map_err(|error| CommandFailureDto {
                    error: PublicErrorDto::BootstrapUnavailable,
                    diagnostic: Some(DiagnosticDto {
                        code: "write_gate_poisoned".into(),
                        message: error.to_string(),
                    }),
                })?;
            self.publish_changed();
        }
        Ok(BootstrapSnapshotDto::from(&snapshot))
    }

    /// Re-resolve the snapshot and emit `bootstrap://changed` with the current
    /// gate generation; used after gate transitions and by the run loop.
    pub fn publish_changed(&self) {
        let snapshot = self.service.inspect();
        self.emitter.emit_changed(&BootstrapChangedPayloadDto {
            snapshot: BootstrapSnapshotDto::from(&snapshot),
            generation: self.write_gate.generation(),
        });
    }
}

impl From<&BootstrapSnapshot> for BootstrapSnapshotDto {
    fn from(snapshot: &BootstrapSnapshot) -> Self {
        match snapshot {
            BootstrapSnapshot::AppStateUnavailable { diagnostic } => {
                BootstrapSnapshotDto::AppStateUnavailable {
                    diagnostic: Some(DiagnosticDto::from(diagnostic)),
                }
            }
            BootstrapSnapshot::Unconfigured => BootstrapSnapshotDto::Unconfigured,
            BootstrapSnapshot::Abandoned { home_id, path } => BootstrapSnapshotDto::Abandoned {
                home_id: home_id.0.clone(),
                path: path.to_string_lossy().into_owned(),
            },
            BootstrapSnapshot::LegacyDetected { path } => BootstrapSnapshotDto::LegacyDetected {
                path: path.to_string_lossy().into_owned(),
            },
            BootstrapSnapshot::FixtureRecoveryLocked { home_id, path } => {
                BootstrapSnapshotDto::FixtureRecoveryLocked {
                    home_id: home_id.as_ref().map(|id| id.0.clone()),
                    path: path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                }
            }
            BootstrapSnapshot::HomeCandidatePending { path, operation_id } => {
                BootstrapSnapshotDto::HomeCandidatePending {
                    path: path.to_string_lossy().into_owned(),
                    operation_id: operation_id.clone(),
                }
            }
            BootstrapSnapshot::Bound {
                home_id,
                catalog_access,
                snapshot_version,
            } => {
                let (catalog_access, catalog_readonly_reason) = match catalog_access {
                    CatalogAccess::ReadWrite => (CatalogAccessDto::ReadWrite, None),
                    CatalogAccess::ReadOnly { reason } => (
                        CatalogAccessDto::ReadOnly,
                        Some(CatalogReadOnlyReasonDto::from(reason)),
                    ),
                };
                BootstrapSnapshotDto::Bound {
                    home_id: home_id.0.clone(),
                    catalog_access,
                    catalog_readonly_reason,
                    snapshot_version: *snapshot_version,
                }
            }
            BootstrapSnapshot::HomeUnavailable {
                home_id,
                path,
                diagnostic,
            } => BootstrapSnapshotDto::HomeUnavailable {
                home_id: home_id.0.clone(),
                path: path.to_string_lossy().into_owned(),
                diagnostic: diagnostic.as_ref().map(DiagnosticDto::from),
            },
            BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                path,
                diagnostic,
            } => BootstrapSnapshotDto::HomeIdentityMismatch {
                home_id: home_id.0.clone(),
                path: path.to_string_lossy().into_owned(),
                diagnostic: diagnostic.as_ref().map(DiagnosticDto::from),
            },
        }
    }
}

impl From<&BootstrapDiagnostic> for DiagnosticDto {
    fn from(diagnostic: &BootstrapDiagnostic) -> Self {
        Self {
            code: diagnostic.code.clone(),
            message: diagnostic.message.clone(),
        }
    }
}

impl From<&crate::core::write_gate::ReadOnlyReason> for CatalogReadOnlyReasonDto {
    fn from(reason: &crate::core::write_gate::ReadOnlyReason) -> Self {
        match reason {
            crate::core::write_gate::ReadOnlyReason::UnsupportedSchema => {
                CatalogReadOnlyReasonDto::UnsupportedSchema
            }
            crate::core::write_gate::ReadOnlyReason::IntegrityFailed => {
                CatalogReadOnlyReasonDto::IntegrityFailed
            }
            crate::core::write_gate::ReadOnlyReason::OpenFailed => {
                CatalogReadOnlyReasonDto::OpenFailed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use crate::core::bootstrap::{BootstrapConfig, BootstrapDiagnostic};
    use crate::core::home::{BoundHome, HomeId};
    use crate::core::write_gate::{ReadOnlyReason, WriteGateState};
    use crate::seams::app_state_store::{AppStateFiles, HomeBindingFile, HomeBindingRecord};
    use crate::seams::catalog_probe::{
        CatalogHomeIdentity, CatalogProbe, CatalogProbeError, CatalogProbeReport,
    };
    use crate::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

    use super::*;

    struct CapturingEmitter {
        payloads: Mutex<Vec<BootstrapChangedPayloadDto>>,
    }

    impl BootstrapChangedEmitter for CapturingEmitter {
        fn emit_changed(&self, payload: &BootstrapChangedPayloadDto) {
            self.payloads.lock().unwrap().push(payload.clone());
        }
    }

    struct PathAppStateStore(PathBuf);

    impl crate::seams::app_state_store::AppStateStore for PathAppStateStore {
        fn load(&self) -> Result<AppStateFiles, crate::seams::app_state_store::AppStateStoreError> {
            Ok(AppStateFiles {
                binding: HomeBindingFile {
                    schema_version: 1,
                    current: Some(HomeBindingRecord {
                        home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
                        path: self.0.clone(),
                        volume_fsid: "fsid-1".into(),
                        volume_uuid: "uuid-1".into(),
                        bound_at: "2026-08-01T00:00:00Z".into(),
                    }),
                    abandoned: vec![],
                },
                recovery_ledger: crate::seams::app_state_store::RecoveryLedgerFile::empty(),
            })
        }

        fn write_locator(
            &self,
            _binding: &HomeBindingFile,
        ) -> Result<(), crate::seams::app_state_store::AppStateStoreError> {
            Ok(())
        }

        fn write_recovery_ledger(
            &self,
            _ledger: &crate::seams::app_state_store::RecoveryLedgerFile,
        ) -> Result<(), crate::seams::app_state_store::AppStateStoreError> {
            Ok(())
        }
    }

    struct FixedVolume(Option<crate::core::home::VolumeIdentity>);

    impl VolumeIdentitySource for FixedVolume {
        fn volume_identity(
            &self,
            _path: &std::path::Path,
        ) -> Result<Option<crate::core::home::VolumeIdentity>, VolumeIdentityError> {
            Ok(self.0.clone())
        }
    }

    struct FixedProbe(CatalogProbeReport);

    impl CatalogProbe for FixedProbe {
        fn probe(&self, _path: &std::path::Path) -> Result<CatalogProbeReport, CatalogProbeError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn snapshot_dto_serializes_the_closed_union_with_snake_case_states() {
        let snapshot = BootstrapSnapshot::AppStateUnavailable {
            diagnostic: BootstrapDiagnostic {
                code: "locator_invalid".into(),
                message: "raw detail".into(),
            },
        };
        let json = serde_json::to_string(&BootstrapSnapshotDto::from(&snapshot)).expect("json");
        assert!(json.contains("\"state\":\"app_state_unavailable\""));
        assert!(json.contains("\"code\":\"locator_invalid\""));
        assert!(json.contains("\"message\":\"raw detail\""));

        let bound = BootstrapSnapshot::Bound {
            home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
            catalog_access: CatalogAccess::ReadOnly {
                reason: ReadOnlyReason::IntegrityFailed,
            },
            snapshot_version: 3,
        };
        let json = serde_json::to_string(&BootstrapSnapshotDto::from(&bound)).expect("json");
        assert!(json.contains("\"state\":\"bound\""));
        assert!(json.contains("\"catalogAccess\":\"read_only\""));
        assert!(json.contains("\"catalogReadonlyReason\":\"integrity_failed\""));
        assert!(json.contains("\"snapshotVersion\":3"));

        let unconfigured = BootstrapSnapshot::Unconfigured;
        let json = serde_json::to_string(&BootstrapSnapshotDto::from(&unconfigured)).expect("json");
        assert!(json.contains("\"state\":\"unconfigured\""));

        let abandoned = BootstrapSnapshot::Abandoned {
            home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
            path: std::path::PathBuf::from("/tmp/skill-man"),
        };
        let json = serde_json::to_string(&BootstrapSnapshotDto::from(&abandoned)).expect("json");
        assert!(json.contains("\"state\":\"abandoned\""));
        assert!(json.contains("\"homeId\":\"b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab\""));
        assert!(json.contains("\"path\":\"/tmp/skill-man\""));

        let unavailable = BootstrapSnapshot::HomeUnavailable {
            home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
            path: std::path::PathBuf::from("/Volumes/Offline/skill-man"),
            diagnostic: Some(BootstrapDiagnostic {
                code: "volume_unreachable".into(),
                message: "volume offline".into(),
            }),
        };
        let json = serde_json::to_string(&BootstrapSnapshotDto::from(&unavailable)).expect("json");
        assert!(json.contains("\"state\":\"home_unavailable\""));
        assert!(json.contains("\"code\":\"volume_unreachable\""));

        let mismatch = BootstrapSnapshot::HomeIdentityMismatch {
            home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
            path: std::path::PathBuf::from("/Volumes/Other/skill-man"),
            diagnostic: Some(BootstrapDiagnostic {
                code: "volume_identity_mismatch".into(),
                message: "different volume".into(),
            }),
        };
        let json = serde_json::to_string(&BootstrapSnapshotDto::from(&mismatch)).expect("json");
        assert!(json.contains("\"state\":\"home_identity_mismatch\""));
        assert!(json.contains("\"code\":\"volume_identity_mismatch\""));
    }

    #[test]
    fn get_snapshot_aligns_the_gate_and_emits_only_on_change() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).expect("home dir");
        std::fs::write(
            home.join(crate::core::home::HomeMarker::FILE_NAME),
            r#"{
                "schema_version": 1,
                "home_id": "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
                "volume_fsid": "fsid-1",
                "volume_uuid": "uuid-1",
                "created_at": "2026-08-01T00:00:00Z"
            }"#,
        )
        .expect("marker file");
        let emitter = Arc::new(CapturingEmitter {
            payloads: Mutex::new(Vec::new()),
        });
        let write_gate = Arc::new(WriteGate::new(WriteGateState::Closed {
            reason: crate::core::write_gate::ClosedReason::Unconfigured,
        }));
        let filesystem = Arc::new(crate::adapters::macos_fs::MacOsFileSystem::new(
            dir.path().to_path_buf(),
        ));
        let classifier = Arc::new(crate::core::fixture_recovery::FixedCleanClassifier);
        let service = Arc::new(BootstrapService::new(
            Arc::new(PathAppStateStore(home.clone())),
            Arc::new(FixedVolume(Some(crate::core::home::VolumeIdentity {
                fsid: "fsid-1".into(),
                uuid: "uuid-1".into(),
            }))),
            Arc::new(FixedProbe(CatalogProbeReport {
                exists: true,
                schema_version: Some(5),
                integrity_ok: true,
                foreign_keys_ok: true,
                home_identity: Some(CatalogHomeIdentity {
                    home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
                    volume_fsid: "fsid-1".into(),
                    volume_uuid: "uuid-1".into(),
                    home_bound_at: "2026-08-01T00:00:00Z".into(),
                }),
                snapshot_version: Some(7),
            })),
            filesystem,
            classifier,
            BootstrapConfig {
                state_dir: PathBuf::from("/tmp/state"),
                default_home_path: PathBuf::from("/tmp/default-home"),
                catalog_file_name: "skill-man.sqlite3".into(),
            },
        ));

        let api = BootstrapApi::new(service, write_gate.clone(), emitter.clone());
        let first = api.get_bootstrap_snapshot().expect("first snapshot");
        assert!(matches!(first, BootstrapSnapshotDto::Bound { .. }));
        // Closed → Open transitioned and emitted once.
        assert!(matches!(
            write_gate.snapshot().state,
            WriteGateState::Open(BoundHome { .. })
        ));
        assert_eq!(write_gate.generation(), 1);
        assert_eq!(emitter.payloads.lock().unwrap().len(), 1);
        let payload = emitter.payloads.lock().unwrap()[0].clone();
        assert!(matches!(
            payload.snapshot,
            BootstrapSnapshotDto::Bound { .. }
        ));
        assert_eq!(payload.generation, 1);

        // A second query sees no state change: no transition, no emit.
        let second = api.get_bootstrap_snapshot().expect("second snapshot");
        assert_eq!(first, second);
        assert_eq!(write_gate.generation(), 1);
        assert_eq!(emitter.payloads.lock().unwrap().len(), 1);
    }
}
