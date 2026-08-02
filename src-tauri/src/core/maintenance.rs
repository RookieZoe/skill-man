use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use thiserror::Error;

use crate::core::domain::ActivationObservedState;
use crate::seams::activation_store::{
    ActivationObservation, ActivationStore, ActivationStoreError,
};
use crate::seams::filesystem::{ActivationEntrySnapshot, FileSystem, FileSystemError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationHealthReport {
    pub checked: u32,
    pub snapshot_version: u64,
}

#[derive(Debug, Error)]
pub enum MaintenanceError {
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error(transparent)]
    Store(#[from] ActivationStoreError),
    #[error("internal Maintenance error: {0}")]
    Internal(String),
}

#[derive(Clone)]
pub struct MaintenanceService {
    store: Arc<dyn ActivationStore>,
    filesystem: Arc<dyn FileSystem>,
}

impl MaintenanceService {
    pub fn new(store: Arc<dyn ActivationStore>, filesystem: Arc<dyn FileSystem>) -> Self {
        Self { store, filesystem }
    }

    pub fn begin_startup(self) -> StartupMaintenance {
        let worker_service = self.clone();
        let worker = std::thread::spawn(move || worker_service.startup_check());
        StartupMaintenance {
            maintenance: self,
            startup_worker: Mutex::new(Some(worker)),
        }
    }

    pub fn startup_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
        let desired = self.store.desired_activations()?;
        let observations = desired
            .iter()
            .map(|activation| {
                let observed_state = match self
                    .filesystem
                    .activation_snapshot(&activation.expected_entry_path)?
                {
                    ActivationEntrySnapshot::Missing => ActivationObservedState::Missing,
                    ActivationEntrySnapshot::Other => ActivationObservedState::Occupied,
                    ActivationEntrySnapshot::Symlink { target }
                        if target != activation.expected_target_path =>
                    {
                        ActivationObservedState::TargetMismatch
                    }
                    ActivationEntrySnapshot::Symlink { .. } => {
                        if self
                            .filesystem
                            .skill_directory_is_readable(&activation.expected_target_path)?
                        {
                            ActivationObservedState::Present
                        } else {
                            ActivationObservedState::Dangling
                        }
                    }
                };
                Ok(ActivationObservation {
                    skill_id: activation.skill_id.clone(),
                    agent_id: activation.agent_id.clone(),
                    observed_state,
                })
            })
            .collect::<Result<Vec<_>, MaintenanceError>>()?;
        let snapshot_version = self.store.record_observations(&observations)?;
        Ok(ActivationHealthReport {
            checked: u32::try_from(observations.len()).map_err(|_| {
                MaintenanceError::Internal("Activation health result exceeds u32 range".into())
            })?,
            snapshot_version,
        })
    }
}

pub struct StartupMaintenance {
    maintenance: MaintenanceService,
    startup_worker: Mutex<Option<JoinHandle<Result<ActivationHealthReport, MaintenanceError>>>>,
}

impl StartupMaintenance {
    pub fn run_activation_health_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
        let mut startup_worker = self
            .startup_worker
            .lock()
            .map_err(|_| MaintenanceError::Internal("startup Maintenance lock poisoned".into()))?;
        if let Some(worker) = startup_worker.take() {
            return worker.join().unwrap_or_else(|_| {
                Err(MaintenanceError::Internal(
                    "startup Maintenance task panicked".into(),
                ))
            });
        }
        drop(startup_worker);
        self.maintenance.startup_check()
    }
}
