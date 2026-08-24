//! Existing Home Recovery Profile (issue #66): a narrow, read-only boundary
//! for inspecting an explicitly chosen complete Home after its bootstrap
//! locator was lost. It never creates, migrates, cleans or binds content.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use thiserror::Error;

use crate::core::bootstrap::{BootstrapService, BootstrapSnapshot};
use crate::core::fixture_recovery::{FixtureClassifier, FixtureShapeMode};
use crate::core::home::{HomeId, HomeMarker};
use crate::core::home_binding::STANDARD_LAYOUT_DIRS;
use crate::seams::app_state_store::{AppStateStore, AppStateStoreError};
use crate::seams::catalog_probe::{CatalogProbe, CatalogProbeError};
use crate::seams::filesystem::FileSystem;

#[derive(Clone, Debug)]
pub struct ExistingHomeRecoveryConfig {
    pub catalog_file_name: String,
}

/// The user-visible facts of a verified Recovery Profile. They are immutable
/// and contain no Home content, credentials or token material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryProfileFacts {
    pub marker_catalog_identity: bool,
    pub standard_layout: bool,
    pub catalog_integrity: bool,
    pub catalog_foreign_keys: bool,
    pub catalog_capabilities: bool,
}

/// One-use preview data. #67 consumes the opaque token for revalidation and
/// locator CAS; this ticket only prepares and cancels the read-only plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExistingHomeRecoveryPlan {
    pub path: PathBuf,
    pub home_id: HomeId,
    pub created_at: String,
    pub plan_token: String,
    pub facts: RecoveryProfileFacts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryEligibilityRejection {
    CurrentBinding,
    AbandonedHistory,
    ActiveRecoveryLedger,
    BootstrapState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryProfileRejection {
    NotDirectory,
    MarkerMissingOrInvalid,
    LayoutCapabilities,
    CatalogMissing,
    CatalogUnreadable,
    CatalogIdentityMissing,
    HomeIdentityMismatch,
    CreationTimeMismatch,
    CatalogIntegrity,
    CatalogForeignKeys,
    CatalogCapabilities,
    ActiveWriter,
    OperationRecoveryRequired,
    FixtureContamination,
}

#[derive(Debug, Error)]
pub enum ExistingHomeRecoveryError {
    #[error("Existing Home Recovery is closed: {reason:?}")]
    Ineligible {
        reason: RecoveryEligibilityRejection,
    },
    #[error("the selected Home does not pass Recovery Profile: {reason:?}")]
    ProfileRejected { reason: RecoveryProfileRejection },
    #[error("the App-level state could not be read: {0}")]
    StateStore(String),
    #[error("the selected Home could not be inspected: {0}")]
    FileSystem(String),
    #[error("the Existing Home Recovery Plan is no longer available")]
    PlanStale,
}

impl From<AppStateStoreError> for ExistingHomeRecoveryError {
    fn from(error: AppStateStoreError) -> Self {
        Self::StateStore(error.to_string())
    }
}

pub struct ExistingHomeRecoveryService {
    app_state: Arc<dyn AppStateStore>,
    bootstrap: Arc<BootstrapService>,
    probe: Arc<dyn CatalogProbe>,
    filesystem: Arc<dyn FileSystem>,
    classifier: Arc<dyn FixtureClassifier>,
    config: ExistingHomeRecoveryConfig,
    plans: RwLock<HashMap<String, ExistingHomeRecoveryPlan>>,
}

impl ExistingHomeRecoveryService {
    pub fn new(
        app_state: Arc<dyn AppStateStore>,
        bootstrap: Arc<BootstrapService>,
        probe: Arc<dyn CatalogProbe>,
        filesystem: Arc<dyn FileSystem>,
        classifier: Arc<dyn FixtureClassifier>,
        config: ExistingHomeRecoveryConfig,
    ) -> Self {
        Self {
            app_state,
            bootstrap,
            probe,
            filesystem,
            classifier,
            config,
            plans: RwLock::new(HashMap::new()),
        }
    }

    /// Prepare an Existing Home Recovery Plan. This only reads the selected
    /// directory and App-level state; it deliberately has no locator, Catalog
    /// or recovery-ledger write path.
    pub fn prepare(
        &self,
        selected_path: &Path,
    ) -> Result<ExistingHomeRecoveryPlan, ExistingHomeRecoveryError> {
        self.ensure_unconfigured()?;
        let path = self
            .filesystem
            .canonical_directory(selected_path)
            .map_err(|_| ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::NotDirectory,
            })?;
        let marker = self.read_marker(&path)?;
        if !self.has_standard_layout(&path) {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::LayoutCapabilities,
            });
        }
        let catalog_path = path.join(&self.config.catalog_file_name);
        let profile = self
            .probe
            .probe_recovery_profile(&catalog_path)
            .map_err(profile_probe_error)?;
        if !profile.exists {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::CatalogMissing,
            });
        }
        let Some(identity) = profile.identity else {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::CatalogIdentityMissing,
            });
        };
        if marker.home_id != identity.home_id {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::HomeIdentityMismatch,
            });
        }
        if marker.created_at != identity.created_at {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::CreationTimeMismatch,
            });
        }
        if !profile.integrity_ok {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::CatalogIntegrity,
            });
        }
        if !profile.foreign_keys_ok {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::CatalogForeignKeys,
            });
        }
        if !profile.required_capabilities {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::CatalogCapabilities,
            });
        }
        if self.writer_is_active(&catalog_path) {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::ActiveWriter,
            });
        }
        if self.operation_recovery_required(&path) {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::OperationRecoveryRequired,
            });
        }
        if self
            .classifier
            .classify(&path, FixtureShapeMode::Bound)
            .is_contaminated()
        {
            return Err(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::FixtureContamination,
            });
        }

        let plan = ExistingHomeRecoveryPlan {
            path,
            home_id: marker.home_id,
            created_at: marker.created_at,
            plan_token: format!("ehr-{:x}", token_nanos()),
            facts: RecoveryProfileFacts {
                marker_catalog_identity: true,
                standard_layout: true,
                catalog_integrity: true,
                catalog_foreign_keys: true,
                catalog_capabilities: true,
            },
        };
        self.plans
            .write()
            .map_err(|_| ExistingHomeRecoveryError::StateStore("plan table poisoned".into()))?
            .insert(plan.plan_token.clone(), plan.clone());
        Ok(plan)
    }

    /// Dropping a preview removes only in-memory opaque state. There is no
    /// durable artifact to clean because preparation was zero-write.
    pub fn cancel(&self, plan_token: &str) -> Result<(), ExistingHomeRecoveryError> {
        let removed = self
            .plans
            .write()
            .map_err(|_| ExistingHomeRecoveryError::StateStore("plan table poisoned".into()))?
            .remove(plan_token);
        removed
            .map(|_| ())
            .ok_or(ExistingHomeRecoveryError::PlanStale)
    }

    fn ensure_unconfigured(&self) -> Result<(), ExistingHomeRecoveryError> {
        let files = self.app_state.load()?;
        if files.binding.current.is_some() {
            return Err(ExistingHomeRecoveryError::Ineligible {
                reason: RecoveryEligibilityRejection::CurrentBinding,
            });
        }
        if !files.binding.abandoned.is_empty() {
            return Err(ExistingHomeRecoveryError::Ineligible {
                reason: RecoveryEligibilityRejection::AbandonedHistory,
            });
        }
        if files.recovery_ledger.active.is_some() {
            return Err(ExistingHomeRecoveryError::Ineligible {
                reason: RecoveryEligibilityRejection::ActiveRecoveryLedger,
            });
        }
        if self.bootstrap.inspect() != BootstrapSnapshot::Unconfigured {
            return Err(ExistingHomeRecoveryError::Ineligible {
                reason: RecoveryEligibilityRejection::BootstrapState,
            });
        }
        Ok(())
    }

    fn read_marker(&self, path: &Path) -> Result<HomeMarker, ExistingHomeRecoveryError> {
        let content = self
            .filesystem
            .read_utf8_file(&path.join(HomeMarker::FILE_NAME))
            .map_err(|error| ExistingHomeRecoveryError::FileSystem(error.to_string()))?;
        content
            .as_deref()
            .and_then(HomeMarker::parse_recovery_profile)
            .ok_or(ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::MarkerMissingOrInvalid,
            })
    }

    /// WAL and SHM sidecars are normal SQLite artifacts. Only a busy or
    /// unprobeable WAL-index lock closes recovery; an acquired lock is
    /// immediately released by the filesystem adapter and leaves no Home
    /// content change behind.
    fn writer_is_active(&self, catalog_path: &Path) -> bool {
        let Some(file_name) = catalog_path.file_name() else {
            return true;
        };
        let mut shm_name = file_name.to_os_string();
        shm_name.push("-shm");
        let shm_path = catalog_path.with_file_name(shm_name);
        match self.filesystem.path_is_occupied(&shm_path) {
            Ok(false) => false,
            Ok(true) => !self
                .filesystem
                .try_lock_wal_index_exclusive(&shm_path)
                .unwrap_or(false),
            Err(_) => true,
        }
    }

    /// A complete Home may retain empty operation roots. Any entry beneath the
    /// owned `staging` or `operations` roots is unfinished evidence and must
    /// continue through the operation-recovery route, never direct recovery.
    fn operation_recovery_required(&self, home_path: &Path) -> bool {
        ["staging", "operations"].iter().any(|name| {
            let root = home_path.join(name);
            match self.filesystem.path_is_directory(&root) {
                Ok(false) => false,
                Ok(true) => self
                    .filesystem
                    .list_directory(&root)
                    .map(|entries| !entries.is_empty())
                    .unwrap_or(true),
                Err(_) => true,
            }
        })
    }

    /// The recovery profile accepts only the exact Home layout produced by
    /// Home Binding. A missing or unreadable root is a closed, zero-write
    /// rejection; empty `operations` and `staging` roots are handled below.
    fn has_standard_layout(&self, home_path: &Path) -> bool {
        STANDARD_LAYOUT_DIRS.iter().all(|directory| {
            self.filesystem
                .path_is_directory(&home_path.join(directory))
                .unwrap_or(false)
        })
    }
}

fn profile_probe_error(error: CatalogProbeError) -> ExistingHomeRecoveryError {
    match error {
        CatalogProbeError::Unreadable(_) | CatalogProbeError::Invalid(_) => {
            ExistingHomeRecoveryError::ProfileRejected {
                reason: RecoveryProfileRejection::CatalogUnreadable,
            }
        }
    }
}

fn token_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}
