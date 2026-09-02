//! Observation Tauri API: `get_observation_snapshot`, `refresh_detection` and
//! the `observation://changed` event (spec §4.10; ADR-0020). The event
//! payload is the same bounded DTO the query returns, and it is published
//! exactly once per Detection Run generation.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{AppHandle, Emitter};

use crate::core::observation::ObservationService;
use crate::core::scan::ScanTrigger;
use crate::core::scan::{ScanError, ScanRunObserver, ScanRunSnapshot};
use crate::tauri_adapter::dto::ObservationAndScanSnapshotDto;

pub const OBSERVATION_CHANGED_EVENT: &str = "observation://changed";

/// Emitter seam so API tests capture payloads without a Tauri runtime.
pub trait ObservationChangedEmitter: Send + Sync {
    fn emit_changed(&self, payload: &ObservationAndScanSnapshotDto);
}

pub struct TauriObservationChangedEmitter {
    app: AppHandle,
}

impl TauriObservationChangedEmitter {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl ObservationChangedEmitter for TauriObservationChangedEmitter {
    fn emit_changed(&self, payload: &ObservationAndScanSnapshotDto) {
        let _ = self.app.emit(OBSERVATION_CHANGED_EVENT, payload);
    }
}

pub struct ObservationApi {
    service: Arc<ObservationService>,
    emitter: Arc<dyn ObservationChangedEmitter>,
    /// Last published Detection generation this API instance has emitted;
    /// concurrent callers publish each generation exactly once.
    last_detection_generation: AtomicU64,
}

impl ObservationApi {
    pub fn new(
        service: Arc<ObservationService>,
        emitter: Arc<dyn ObservationChangedEmitter>,
    ) -> Self {
        Self {
            service,
            emitter,
            last_detection_generation: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> ObservationAndScanSnapshotDto {
        self.service.snapshot().into()
    }

    /// Single-flight Detection trigger; publishes `observation://changed`
    /// when the Run advanced the Detection generation.
    pub fn refresh_detection(&self) -> ObservationAndScanSnapshotDto {
        let dto: ObservationAndScanSnapshotDto = self.service.refresh_detection().into();
        let generation = dto.detection.generation;
        let previous = self.last_detection_generation.load(Ordering::Acquire);
        if generation != previous
            && self
                .last_detection_generation
                .compare_exchange(previous, generation, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.emitter.emit_changed(&dto);
        }
        dto
    }

    /// Start a full Rescan Run (single-flight; spec §4.10). The Run emits
    /// `observation://changed` through the same observer channel as the
    /// Detection results.
    pub fn start_rescan(
        &self,
        trigger: ScanTrigger,
    ) -> Result<ObservationAndScanSnapshotDto, ScanError> {
        let dto: ObservationAndScanSnapshotDto = self.service.start_rescan(trigger)?.into();
        // State/phase changes already published through the observer; the
        // response itself carries the fresh snapshot.
        Ok(dto)
    }

    /// Coordinate cancellation of the active Run (spec §4.10).
    pub fn cancel_rescan(&self, run_id: &str) -> Result<ObservationAndScanSnapshotDto, ScanError> {
        let dto: ObservationAndScanSnapshotDto = self.service.cancel_rescan(run_id)?.into();
        Ok(dto)
    }
}

/// The API is the scan progress observer: the coordinator throttles to
/// ≤250 ms and forces phase/state changes; the API converts to the shared
/// `observation://changed` payload.
impl ScanRunObserver for ObservationApi {
    fn on_scan_change(&self, _snapshot: &ScanRunSnapshot) {
        let dto = self.snapshot();
        self.emitter.emit_changed(&dto);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::core::agent_configuration::PresetRegistry;
    use crate::core::home::BoundHome;
    use crate::core::observation::ObservationService;
    use crate::core::write_gate::{WriteGate, WriteGateState};
    use crate::seams::agent_configuration_fs::{
        AgentConfigurationFileSystem, AgentConfigurationFileSystemError, AgentRootInspection,
        AgentRootProbe, CreatedAgentTargetDirectory,
    };
    use crate::seams::agent_configuration_store::{
        AgentConfigurationStore, AgentConfigurationStoreChange, AgentConfigurationStoreError,
        AgentConfigurationStoreSnapshot, RecentProjectFolder, StoredAgentConfiguration,
        StoredGlobalSkillRoot,
    };
    use parking_lot::Mutex;

    #[derive(Clone, Default)]
    struct CapturingEmitter {
        payloads: Arc<Mutex<Vec<ObservationAndScanSnapshotDto>>>,
    }

    impl ObservationChangedEmitter for CapturingEmitter {
        fn emit_changed(&self, payload: &ObservationAndScanSnapshotDto) {
            self.payloads.lock().push(payload.clone());
        }
    }

    struct PresentProbe;

    impl AgentConfigurationFileSystem for PresentProbe {
        fn inspect_root(
            &self,
            _configured_path: &Path,
        ) -> Result<AgentRootInspection, AgentConfigurationFileSystemError> {
            unreachable!("detection never inspects roots")
        }

        fn probe_root(
            &self,
            _configured_path: &Path,
        ) -> Result<AgentRootProbe, AgentConfigurationFileSystemError> {
            Ok(AgentRootProbe::Present {
                canonical_path: std::path::PathBuf::from("/Users/me/.root"),
            })
        }

        fn create_target(
            &self,
            _planned: &AgentRootInspection,
        ) -> Result<CreatedAgentTargetDirectory, AgentConfigurationFileSystemError> {
            unreachable!("detection never creates targets")
        }

        fn rollback_created_target(
            &self,
            _receipt: &CreatedAgentTargetDirectory,
        ) -> Result<(), AgentConfigurationFileSystemError> {
            unreachable!("detection never creates targets")
        }

        fn random_bytes(
            &self,
            _buffer: &mut [u8],
        ) -> Result<(), AgentConfigurationFileSystemError> {
            unreachable!("detection never draws entropy")
        }
    }

    #[derive(Clone)]
    struct StubStore(Arc<Mutex<u64>>);

    impl AgentConfigurationStore for StubStore {
        fn agent_configuration_snapshot(
            &self,
        ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError> {
            Ok(AgentConfigurationStoreSnapshot {
                snapshot_version: *self.0.lock(),
                configurations: Vec::<StoredAgentConfiguration>::new(),
                roots: Vec::<StoredGlobalSkillRoot>::new(),
            })
        }

        fn apply_agent_configuration_change(
            &self,
            _expected_snapshot_version: u64,
            _change: AgentConfigurationStoreChange,
        ) -> Result<u64, AgentConfigurationStoreError> {
            unreachable!("observation never applies configuration changes")
        }

        fn list_recent_project_folders(
            &self,
        ) -> Result<Vec<RecentProjectFolder>, AgentConfigurationStoreError> {
            unreachable!("observation never reads project folders")
        }

        fn record_recent_project_folder(
            &self,
            _folder: RecentProjectFolder,
        ) -> Result<(), AgentConfigurationStoreError> {
            unreachable!("observation never records project folders")
        }

        fn clear_recent_project_folders(&self) -> Result<(), AgentConfigurationStoreError> {
            unreachable!("observation never clears project folders")
        }
    }

    fn api() -> (ObservationApi, CapturingEmitter) {
        let gate = Arc::new(WriteGate::new(WriteGateState::Open(BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            std::path::PathBuf::from("/tmp/skill-man-home"),
        ))));
        let service = Arc::new(ObservationService::new(
            Arc::new(PresentProbe),
            PresetRegistry::system(),
            gate,
            Arc::new(StubStore(Arc::new(Mutex::new(7)))),
        ));
        let emitter = CapturingEmitter::default();
        (
            ObservationApi::new(service, Arc::new(emitter.clone())),
            emitter,
        )
    }

    #[test]
    fn refresh_publishes_each_generation_on_the_changed_event() {
        let (api, emitter) = api();
        let first = api.refresh_detection();
        assert_eq!(first.detection.generation, 1);
        assert_eq!(emitter.payloads.lock().len(), 1);
        let second = api.refresh_detection();
        assert_eq!(second.detection.generation, 2);
        assert_eq!(emitter.payloads.lock().len(), 2);
    }

    #[test]
    fn concurrent_callers_publish_one_event_per_generation() {
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let probe = GatedProbe {
            started: started_tx,
            gate: Arc::new(parking_lot::Mutex::new(Some(release_rx))),
        };
        let service = Arc::new(ObservationService::new(
            Arc::new(probe),
            PresetRegistry::system(),
            Arc::new(WriteGate::new(WriteGateState::Open(BoundHome::test_value(
                "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
                std::path::PathBuf::from("/tmp/skill-man-home"),
            )))),
            Arc::new(StubStore(Arc::new(Mutex::new(3)))),
        ));
        let emitter = CapturingEmitter::default();
        let api = Arc::new(ObservationApi::new(service, Arc::new(emitter.clone())));
        let mut joins = Vec::new();
        for _ in 0..4 {
            let api = api.clone();
            joins.push(std::thread::spawn(move || api.refresh_detection()));
        }
        // Wait until the Run is actively probing, give the three other
        // triggers time to join it, then release the gate.
        started_rx.recv().expect("probe started");
        std::thread::sleep(std::time::Duration::from_millis(50));
        release_tx.send(()).expect("release the active Run");
        for join in joins {
            let _ = join.join().expect("refresh join");
        }
        // One Run was joined by three triggers; exactly one event per
        // generation, and the single-flight run only advanced once.
        assert_eq!(emitter.payloads.lock().len(), 1);
    }

    struct GatedProbe {
        started: std::sync::mpsc::SyncSender<()>,
        gate: Arc<parking_lot::Mutex<Option<std::sync::mpsc::Receiver<()>>>>,
    }

    impl AgentConfigurationFileSystem for GatedProbe {
        fn inspect_root(
            &self,
            _configured_path: &Path,
        ) -> Result<AgentRootInspection, AgentConfigurationFileSystemError> {
            unreachable!("detection never inspects roots")
        }

        fn probe_root(
            &self,
            _configured_path: &Path,
        ) -> Result<AgentRootProbe, AgentConfigurationFileSystemError> {
            if let Some(block) = self.gate.lock().take() {
                let _ = self.started.send(());
                let _ = block.recv();
            }
            Ok(AgentRootProbe::Present {
                canonical_path: std::path::PathBuf::from("/Users/me/.root"),
            })
        }

        fn create_target(
            &self,
            _planned: &AgentRootInspection,
        ) -> Result<CreatedAgentTargetDirectory, AgentConfigurationFileSystemError> {
            unreachable!("detection never creates targets")
        }

        fn rollback_created_target(
            &self,
            _receipt: &CreatedAgentTargetDirectory,
        ) -> Result<(), AgentConfigurationFileSystemError> {
            unreachable!("detection never creates targets")
        }

        fn random_bytes(
            &self,
            _buffer: &mut [u8],
        ) -> Result<(), AgentConfigurationFileSystemError> {
            unreachable!("detection never draws entropy")
        }
    }

    #[test]
    fn snapshot_never_triggers_detection_or_events() {
        let (api, emitter) = api();
        let snapshot = api.snapshot();
        assert_eq!(snapshot.detection.generation, 0);
        assert_eq!(snapshot.agent_configuration_generation, Some(7));
        assert_eq!(emitter.payloads.lock().len(), 0);
    }
}
