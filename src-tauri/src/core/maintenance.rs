use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use thiserror::Error;

use crate::core::domain::{ActivationObservedState, Health};
use crate::seams::activation_store::{ActivationObservation, ActivationStoreError};
use crate::seams::filesystem::{
    ActivationEntrySnapshot, FileImportRecoveryBaseline, FileSystem, FileSystemError,
};
use crate::seams::maintenance_store::{
    MaintenanceStore, MaintenanceStoreError, SkillHealthObservation,
};
use crate::seams::recovery::RecoveryGate;

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
    #[error(transparent)]
    MaintenanceStore(#[from] MaintenanceStoreError),
    #[error("internal Maintenance error: {0}")]
    Internal(String),
}

#[derive(Clone)]
pub struct MaintenanceService {
    store: Arc<dyn MaintenanceStore>,
    filesystem: Arc<dyn FileSystem>,
    library_root: Option<PathBuf>,
    recovery_gate: Arc<RecoveryGate>,
}

impl MaintenanceService {
    pub fn new(store: Arc<dyn MaintenanceStore>, filesystem: Arc<dyn FileSystem>) -> Self {
        Self {
            store,
            filesystem,
            library_root: None,
            recovery_gate: Arc::new(RecoveryGate::ready()),
        }
    }

    pub fn with_library_root(mut self, library_root: PathBuf) -> Self {
        self.library_root = Some(library_root);
        self
    }

    pub fn with_recovery_gate(mut self, recovery_gate: Arc<RecoveryGate>) -> Self {
        self.recovery_gate = recovery_gate;
        self
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
        self.recover_startup_operations()?;
        self.recovery_gate.mark_ready();
        self.run_health_check()
    }

    fn recover_startup_operations(&self) -> Result<(), MaintenanceError> {
        if let Some(library_root) = &self.library_root {
            let baselines = self
                .store
                .installed_skill_baselines()?
                .into_iter()
                .map(|baseline| FileImportRecoveryBaseline {
                    skill_id: baseline.skill_id.0,
                    final_entity_path: baseline.final_entity_path,
                    recorded_content_hash: baseline.recorded_content_hash,
                })
                .collect::<Vec<_>>();
            self.filesystem
                .recover_file_import_journals(library_root, &baselines)?;
        }
        Ok(())
    }

    fn run_health_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
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
        let mut snapshot_version = self.store.record_observations(&observations)?;
        let installed = self.store.installed_skill_baselines()?;
        if !installed.is_empty() {
            let health_observations = installed
                .into_iter()
                .map(|skill| {
                    let health = if !self
                        .filesystem
                        .skill_directory_is_readable(&skill.final_entity_path)?
                    {
                        Health::Broken
                    } else if self.filesystem.tree_hash(&skill.final_entity_path)?
                        == skill.recorded_content_hash
                    {
                        Health::Healthy
                    } else {
                        Health::Modified
                    };
                    Ok(SkillHealthObservation {
                        skill_id: skill.skill_id,
                        health,
                    })
                })
                .collect::<Result<Vec<_>, MaintenanceError>>()?;
            snapshot_version = self.store.record_skill_health(&health_observations)?;
        }
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
            worker.join().unwrap_or_else(|_| {
                Err(MaintenanceError::Internal(
                    "startup Maintenance task panicked".into(),
                ))
            })?;
        }
        drop(startup_worker);
        self.maintenance.run_health_check()
    }
}
