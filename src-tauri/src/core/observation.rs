//! Observation and Scan Module skeleton (spec §4.10; ADR-0020).
//!
//! This vertical slice (#81) implements the Agent Detection part of the joint
//! Core Interface: `snapshot()` and single-flight `refresh_detection()`.
//! Startup Probe, Activation Health, Scan Run and current Report arrive on
//! the same Interface in their own tickets; the module owns the Interface so
//! React, Agent Management and Adopt never compose their own filesystem
//! loops or generations.
//!
//! Detection only observes the nine Preset user-level roots: the result
//! stays in memory, the Catalog is never written and no directory is ever
//! created (spec §10.1 "Preset detected but unconfigured"). A probe failure
//! is `Unavailable`, never `Absent` (ADR-0020: Unavailable 不降级 Absent);
//! configuration from a detection result re-verifies every root through the
//! Agent Configuration plan path.

use std::path::PathBuf;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use crate::core::agent_configuration::PresetRegistry;
use crate::core::write_gate::WriteGate;
use crate::seams::agent_configuration_fs::{
    AgentConfigurationFileSystem, AgentConfigurationFileSystemError, AgentRootProbe,
};
use crate::seams::agent_configuration_store::AgentConfigurationStore;

/// Closed per-root observation state. An unreadable or invalid root is
/// `Unavailable` with a diagnostic — never downgraded to `Absent`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootDetectionState {
    Present,
    Unavailable,
    Absent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootObservation {
    pub configured_path: PathBuf,
    pub state: RootDetectionState,
    /// Resolved directory identity when `Present`.
    pub canonical_path: Option<PathBuf>,
    /// Raw reason when `Unavailable`; never user copy.
    pub diagnostic: Option<String>,
}

/// Closed per-preset observation state. `Unknown` is the honest pre-run
/// state: the detector has not observed this preset yet, and absence of
/// evidence must never be rendered as absence of the agent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresetDetectionState {
    Present,
    Unavailable,
    Absent,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetObservation {
    pub preset_key: String,
    pub name: String,
    pub state: PresetDetectionState,
    pub roots: Vec<RootObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectionSnapshot {
    pub generation: u64,
    pub preset_observations: Vec<PresetObservation>,
}

/// Bounded summary shared as the query result and `observation://changed`
/// event payload (spec §4.10: the two are isomorphic). Startup Probe,
/// Activation Health, Scan Run and current Report are separate upcoming
/// slices on the same Interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationAndScanSnapshot {
    pub home_id: Option<String>,
    pub write_gate_generation: u64,
    pub agent_configuration_generation: Option<u64>,
    pub detection: DetectionSnapshot,
}

/// Single-flight run bookkeeping: exactly one thread may probe; concurrent
/// triggers register a channel and reuse the Run's published result.
enum DetectionRun {
    Idle,
    Running(Vec<SyncSender<DetectionSnapshot>>),
}

struct SharedState {
    run: DetectionRun,
    detection: DetectionSnapshot,
}

pub struct ObservationService {
    filesystem: Arc<dyn AgentConfigurationFileSystem>,
    presets: PresetRegistry,
    write_gate: Arc<WriteGate>,
    agent_store: Arc<dyn AgentConfigurationStore>,
    state: Mutex<SharedState>,
}

impl ObservationService {
    pub fn new(
        filesystem: Arc<dyn AgentConfigurationFileSystem>,
        presets: PresetRegistry,
        write_gate: Arc<WriteGate>,
        agent_store: Arc<dyn AgentConfigurationStore>,
    ) -> Self {
        let detection = DetectionSnapshot {
            generation: 0,
            preset_observations: presets
                .presets()
                .iter()
                .map(|preset| PresetObservation {
                    preset_key: preset.preset_key.clone(),
                    name: preset.name.clone(),
                    state: PresetDetectionState::Unknown,
                    roots: Vec::new(),
                })
                .collect(),
        };
        Self {
            filesystem,
            presets,
            write_gate,
            agent_store,
            state: Mutex::new(SharedState {
                run: DetectionRun::Idle,
                detection,
            }),
        }
    }

    /// Current in-memory view; never probes the filesystem.
    pub fn snapshot(&self) -> ObservationAndScanSnapshot {
        let detection = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .detection
            .clone();
        ObservationAndScanSnapshot {
            home_id: self.write_gate.bound_home().ok().map(|home| home.home_id.0),
            write_gate_generation: self.write_gate.generation(),
            agent_configuration_generation: self
                .agent_store
                .agent_configuration_snapshot()
                .ok()
                .map(|snapshot| snapshot.snapshot_version),
            detection,
        }
    }

    /// Run or join a Detection Run (single-flight, ADR-0020): a trigger while
    /// a Run is active reuses its published result instead of probing again.
    /// The result is published in memory and returned; the caller publishes
    /// the `observation://changed` payload when the generation advanced.
    pub fn refresh_detection(&self) -> ObservationAndScanSnapshot {
        let (owned_run, joined) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &mut state.run {
                DetectionRun::Running(waiters) => {
                    let (sender, receiver) = sync_channel(1);
                    waiters.push(sender);
                    (false, Some(receiver))
                }
                DetectionRun::Idle => {
                    state.run = DetectionRun::Running(Vec::new());
                    (true, None)
                }
            }
        };
        if owned_run {
            let published = {
                let generation = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .detection
                    .generation
                    .wrapping_add(1);
                self.probe_detection(generation)
            };
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.detection = published.clone();
            if let DetectionRun::Running(waiters) = &mut state.run {
                for waiter in waiters.drain(..) {
                    let _ = waiter.send(published.clone());
                }
            }
            state.run = DetectionRun::Idle;
        } else {
            joined
                .expect("a joined Run always has a receiver")
                .recv()
                .expect("an active Detection Run always publishes");
        }
        self.snapshot()
    }

    fn probe_detection(&self, generation: u64) -> DetectionSnapshot {
        let preset_observations = self
            .presets
            .presets()
            .iter()
            .map(|preset| {
                let roots = preset
                    .roots
                    .iter()
                    .map(|configured_path| self.probe_root(configured_path))
                    .collect::<Vec<_>>();
                let state = if roots
                    .iter()
                    .any(|root| root.state == RootDetectionState::Present)
                {
                    PresetDetectionState::Present
                } else if roots
                    .iter()
                    .any(|root| root.state == RootDetectionState::Unavailable)
                {
                    PresetDetectionState::Unavailable
                } else {
                    PresetDetectionState::Absent
                };
                PresetObservation {
                    preset_key: preset.preset_key.clone(),
                    name: preset.name.clone(),
                    state,
                    roots,
                }
            })
            .collect();
        DetectionSnapshot {
            generation,
            preset_observations,
        }
    }

    fn probe_root(&self, configured_path: &std::path::Path) -> RootObservation {
        let outcome = self.filesystem.probe_root(configured_path);
        match outcome {
            Ok(AgentRootProbe::Present { canonical_path }) => RootObservation {
                configured_path: configured_path.to_path_buf(),
                state: RootDetectionState::Present,
                canonical_path: Some(canonical_path),
                diagnostic: None,
            },
            Ok(AgentRootProbe::Unavailable { diagnostic }) => RootObservation {
                configured_path: configured_path.to_path_buf(),
                state: RootDetectionState::Unavailable,
                canonical_path: None,
                diagnostic: Some(diagnostic),
            },
            Ok(AgentRootProbe::Absent) => RootObservation {
                configured_path: configured_path.to_path_buf(),
                state: RootDetectionState::Absent,
                canonical_path: None,
                diagnostic: None,
            },
            Err(error) => RootObservation {
                configured_path: configured_path.to_path_buf(),
                state: RootDetectionState::Unavailable,
                canonical_path: None,
                diagnostic: Some(agent_root_probe_error(&error)),
            },
        }
    }
}

fn agent_root_probe_error(error: &AgentConfigurationFileSystemError) -> String {
    match error {
        AgentConfigurationFileSystemError::InvalidPath => "invalid_path".to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::core::agent_configuration::AgentPreset;
    use crate::core::home::BoundHome;
    use crate::core::write_gate::{WriteGate, WriteGateState};
    use crate::seams::agent_configuration_fs::{AgentRootInspection, CreatedAgentTargetDirectory};
    use crate::seams::agent_configuration_store::{
        AgentConfigurationStoreChange, AgentConfigurationStoreError,
        AgentConfigurationStoreSnapshot, RecentProjectFolder, StoredAgentConfiguration,
        StoredGlobalSkillRoot,
    };

    struct ProbeOutcomes {
        outcomes: std::collections::HashMap<PathBuf, AgentRootProbe>,
        calls: AtomicUsize,
        /// Optional gate: when present, every root probe blocks on `recv`
        /// until the test releases the active Run.
        block: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    struct StubFilesystem {
        outcomes: ProbeOutcomes,
    }

    impl AgentConfigurationFileSystem for StubFilesystem {
        fn inspect_root(
            &self,
            _configured_path: &Path,
        ) -> Result<AgentRootInspection, AgentConfigurationFileSystemError> {
            unreachable!("detection never inspects roots")
        }

        fn probe_root(
            &self,
            configured_path: &Path,
        ) -> Result<AgentRootProbe, AgentConfigurationFileSystemError> {
            self.outcomes.calls.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut guard) = self.outcomes.block.lock()
                && let Some(block) = guard.take()
            {
                let _ = block.recv();
            }
            let outcome = self
                .outcomes
                .outcomes
                .get(configured_path)
                .cloned()
                .unwrap_or(AgentRootProbe::Absent);
            Ok(outcome)
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

    struct StubStore {
        snapshot_version: u64,
    }

    impl AgentConfigurationStore for StubStore {
        fn agent_configuration_snapshot(
            &self,
        ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError> {
            Ok(AgentConfigurationStoreSnapshot {
                snapshot_version: self.snapshot_version,
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

    fn test_presets() -> PresetRegistry {
        PresetRegistry::from_presets(vec![
            test_preset("alpha", "Alpha", vec!["~/.alpha/skills", "~/.root/skills"]),
            test_preset("beta", "Beta", vec!["~/.beta/skills"]),
        ])
    }

    fn test_preset(key: &str, name: &str, roots: Vec<&str>) -> AgentPreset {
        AgentPreset {
            preset_key: key.into(),
            name: name.into(),
            compatibility: crate::core::domain::Compatibility::Verified,
            roots: roots.into_iter().map(PathBuf::from).collect(),
            activation_target: PathBuf::from("~/.target"),
            project_skills_dir: PathBuf::from(".skills"),
        }
    }

    fn service(
        outcomes: &[(&str, AgentRootProbe)],
        snapshot_version: u64,
    ) -> (ObservationService, Arc<StubFilesystem>, Arc<WriteGate>) {
        let filesystem = Arc::new(StubFilesystem {
            outcomes: ProbeOutcomes {
                outcomes: outcomes
                    .iter()
                    .map(|(path, outcome)| (PathBuf::from(path), outcome.clone()))
                    .collect(),
                calls: AtomicUsize::new(0),
                block: Mutex::new(None),
            },
        });
        let gate = Arc::new(WriteGate::new(WriteGateState::Open(BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            PathBuf::from("/tmp/skill-man-home"),
        ))));
        let service = ObservationService::new(
            filesystem.clone(),
            test_presets(),
            gate.clone(),
            Arc::new(StubStore { snapshot_version }),
        );
        (service, filesystem, gate)
    }

    #[test]
    fn fresh_snapshot_is_all_unknown_with_generation_zero() {
        let (service, filesystem, _gate) = service(&[], 7);
        let snapshot = service.snapshot();
        assert_eq!(snapshot.detection.generation, 0);
        assert_eq!(snapshot.detection.preset_observations.len(), 2);
        assert!(
            snapshot
                .detection
                .preset_observations
                .iter()
                .all(|preset| preset.state == PresetDetectionState::Unknown)
        );
        assert_eq!(filesystem.outcomes.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            snapshot.home_id.as_deref(),
            Some("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab")
        );
        assert_eq!(snapshot.write_gate_generation, 0);
        assert_eq!(snapshot.agent_configuration_generation, Some(7));
    }

    #[test]
    fn refresh_detection_publishes_present_unavailable_and_absent_roots() {
        let (service, _filesystem, _gate) = service(
            &[
                (
                    "~/.alpha/skills",
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/.alpha/skills"),
                    },
                ),
                (
                    "~/.beta/skills",
                    AgentRootProbe::Unavailable {
                        diagnostic: "read denied".into(),
                    },
                ),
            ],
            0,
        );
        let snapshot = service.refresh_detection();
        assert_eq!(snapshot.detection.generation, 1);
        let alpha = &snapshot.detection.preset_observations[0];
        let beta = &snapshot.detection.preset_observations[1];
        assert_eq!(alpha.state, PresetDetectionState::Present);
        assert_eq!(alpha.roots.len(), 2);
        assert_eq!(alpha.roots[0].state, RootDetectionState::Present);
        assert_eq!(
            alpha.roots[0].canonical_path.as_deref(),
            Some(Path::new("/Users/me/.alpha/skills"))
        );
        // The shared root is Absent, and a root being absent does not
        // downgrade the preset below the Present one.
        assert_eq!(alpha.roots[1].state, RootDetectionState::Absent);
        assert_eq!(beta.state, PresetDetectionState::Unavailable);
        assert_eq!(beta.roots[0].state, RootDetectionState::Unavailable);
        assert_eq!(beta.roots[0].diagnostic.as_deref(), Some("read denied"));
    }

    #[test]
    fn unavailable_root_is_never_downgraded_to_absent() {
        let (service, _filesystem, _gate) = service(
            &[(
                "~/.beta/skills",
                AgentRootProbe::Unavailable {
                    diagnostic: "permission denied".into(),
                },
            )],
            0,
        );
        let snapshot = service.refresh_detection();
        let beta = &snapshot.detection.preset_observations[1];
        assert_eq!(beta.state, PresetDetectionState::Unavailable);
        assert_eq!(beta.roots[0].state, RootDetectionState::Unavailable);
    }

    #[test]
    fn repeated_triggers_reuse_the_active_run_single_flight() {
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let filesystem = Arc::new(StubFilesystem {
            outcomes: ProbeOutcomes {
                outcomes: [(
                    PathBuf::from("~/.alpha/skills"),
                    AgentRootProbe::Present {
                        canonical_path: PathBuf::from("/Users/me/.alpha/skills"),
                    },
                )]
                .into_iter()
                .collect(),
                calls: AtomicUsize::new(0),
                block: Mutex::new(Some(release_rx)),
            },
        });
        let gate = Arc::new(WriteGate::new(WriteGateState::Open(BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            PathBuf::from("/tmp/skill-man-home"),
        ))));
        let service = Arc::new(ObservationService::new(
            filesystem.clone(),
            test_presets(),
            gate,
            Arc::new(StubStore {
                snapshot_version: 0,
            }),
        ));
        let first = {
            let service_clone = service.clone();
            std::thread::spawn(move || service_clone.refresh_detection())
        };
        // Wait until the first trigger runs the probe, then join a second
        // trigger while the Run is still active (single-flight reuse).
        while filesystem.outcomes.calls.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        let second = {
            let service_clone = service.clone();
            std::thread::spawn(move || service_clone.refresh_detection())
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        release_tx.send(()).expect("release the active probe");
        let first_snapshot = first.join().expect("first join");
        let second_snapshot = second.join().expect("second join");
        assert_eq!(
            first_snapshot.detection.generation,
            second_snapshot.detection.generation
        );
        // One Run probes every preset root once: alpha has two roots, beta one.
        assert_eq!(filesystem.outcomes.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_later_refresh_starts_a_new_run_and_bumps_generation() {
        let (service, filesystem, _gate) = service(
            &[(
                "~/.alpha/skills",
                AgentRootProbe::Present {
                    canonical_path: PathBuf::from("/Users/me/.alpha/skills"),
                },
            )],
            0,
        );
        let first = service.refresh_detection();
        assert_eq!(first.detection.generation, 1);
        let second = service.refresh_detection();
        assert_eq!(second.detection.generation, 2);
        assert_eq!(filesystem.outcomes.calls.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn unavailable_and_closed_store_yield_none_generation_not_errors() {
        let filesystem = Arc::new(StubFilesystem {
            outcomes: ProbeOutcomes {
                outcomes: std::collections::HashMap::new(),
                calls: AtomicUsize::new(0),
                block: Mutex::new(None),
            },
        });
        let gate = Arc::new(WriteGate::new(WriteGateState::Closed {
            reason: crate::core::write_gate::ClosedReason::Unconfigured,
        }));
        // The closed store facade never reaches here; simulate an
        // unavailable store with a failing stub.
        let failing = Arc::new(FailingStore);
        let service = ObservationService::new(filesystem, test_presets(), gate.clone(), failing);
        let snapshot = service.snapshot();
        assert_eq!(snapshot.home_id, None);
        assert_eq!(snapshot.write_gate_generation, 0);
        assert_eq!(snapshot.agent_configuration_generation, None);
    }

    struct FailingStore;

    impl AgentConfigurationStore for FailingStore {
        fn agent_configuration_snapshot(
            &self,
        ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError> {
            Err(AgentConfigurationStoreError::Unavailable("closed".into()))
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
}
