//! Existing Home Recovery (issues #66–#67): an explicitly chosen complete
//! Home can first be inspected read-only, then directly confirmed to rebuild
//! only its lost bootstrap locator. It never creates, migrates, cleans or
//! rewrites Home content.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use thiserror::Error;

use crate::core::bootstrap::{BootstrapService, BootstrapSnapshot};
pub use crate::core::existing_home_profile::RecoveryProfileRejection;
use crate::core::existing_home_profile::{
    ExistingHomeRecoveryProfile, RecoveryProfileInspectionError, VerifiedRecoveryProfile,
};
use crate::core::fixture_recovery::FixtureClassifier;
use crate::core::home::{HomeId, VolumeIdentity};
use crate::core::write_gate::WriteGate;
use crate::seams::app_state_store::{
    AppStateFiles, AppStateStore, AppStateStoreError, HOME_BINDING_SCHEMA_VERSION, HomeBindingFile,
    HomeBindingRecord,
};
use crate::seams::catalog_probe::CatalogProbe;
use crate::seams::filesystem::FileSystem;
use crate::seams::volume_identity::VolumeIdentitySource;

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

/// One-use preview data. Confirmation consumes the opaque token only after
/// revalidation and the locator CAS succeed.
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
    #[error("Existing Home Recovery is blocked while another recovery owns the WriteGate")]
    RecoveryInProgress,
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
    volume: Arc<dyn VolumeIdentitySource>,
    probe: Arc<dyn CatalogProbe>,
    filesystem: Arc<dyn FileSystem>,
    classifier: Arc<dyn FixtureClassifier>,
    write_gate: Arc<WriteGate>,
    config: ExistingHomeRecoveryConfig,
    plans: RwLock<HashMap<String, ExistingHomeRecoveryPlan>>,
}

impl ExistingHomeRecoveryService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app_state: Arc<dyn AppStateStore>,
        bootstrap: Arc<BootstrapService>,
        volume: Arc<dyn VolumeIdentitySource>,
        probe: Arc<dyn CatalogProbe>,
        filesystem: Arc<dyn FileSystem>,
        classifier: Arc<dyn FixtureClassifier>,
        write_gate: Arc<WriteGate>,
        config: ExistingHomeRecoveryConfig,
    ) -> Self {
        Self {
            app_state,
            bootstrap,
            volume,
            probe,
            filesystem,
            classifier,
            write_gate,
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
        let profile = self.inspect_profile(selected_path)?;
        self.ensure_preparable_path(&profile.path)?;

        let plan = ExistingHomeRecoveryPlan {
            path: profile.path,
            home_id: profile.marker_home_id,
            created_at: profile.marker_created_at,
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

    /// Confirming a plan re-runs the complete read-only Recovery Profile,
    /// then atomically recreates only the lost bootstrap locator. Home
    /// content, the recovery ledger and all Catalog data stay untouched.
    /// Once the locator CAS commits, there is no rollback: a later access or
    /// identity failure is resolved by the ordinary Bound/closed bootstrap
    /// state machine.
    pub fn confirm(
        &self,
        plan_token: &str,
    ) -> Result<BootstrapSnapshot, ExistingHomeRecoveryError> {
        let plan = self
            .plans
            .read()
            .map_err(|_| ExistingHomeRecoveryError::StateStore("plan table poisoned".into()))?
            .get(plan_token)
            .cloned()
            .ok_or(ExistingHomeRecoveryError::PlanStale)?;

        let home_transition =
            self.write_gate
                .begin_home_transition()
                .map_err(|error| match error {
                    crate::core::write_gate::WriteGateError::Closed => {
                        ExistingHomeRecoveryError::RecoveryInProgress
                    }
                    other => ExistingHomeRecoveryError::StateStore(other.to_string()),
                })?;
        let result = self.confirm_plan(&plan);
        if result.is_ok() {
            home_transition.commit();
        }
        if result.is_ok() || matches!(result, Err(ExistingHomeRecoveryError::PlanStale)) {
            self.remove_plan(plan_token)?;
        }
        result
    }

    fn confirm_plan(
        &self,
        plan: &ExistingHomeRecoveryPlan,
    ) -> Result<BootstrapSnapshot, ExistingHomeRecoveryError> {
        let files = self.app_state.load()?;
        self.ensure_complete_unconfigured(&files)
            .map_err(|_| ExistingHomeRecoveryError::PlanStale)?;

        // Every fact that made the preview safe is re-probed immediately
        // before CAS. Any replaced path, changed marker/Catalog identity,
        // active WAL writer or newly unfinished operation invalidates the
        // opaque preview rather than creating a partial recovery state.
        let revalidated = self
            .inspect_profile(&plan.path)
            .map_err(|_| ExistingHomeRecoveryError::PlanStale)?;
        if revalidated.path != plan.path
            || revalidated.marker_home_id != plan.home_id
            || revalidated.marker_created_at != plan.created_at
        {
            return Err(ExistingHomeRecoveryError::PlanStale);
        }
        // A plan prepared while the default path was empty must not tunnel
        // through a Default Home Recovery Offer that appeared meanwhile.
        // Re-check the current route against the plan's canonical path just
        // before the locator CAS; state drift is a stale plan, never a
        // permission to recover another selected Home.
        self.ensure_snapshot_allows_path(&plan.path)
            .map_err(|_| ExistingHomeRecoveryError::PlanStale)?;
        let volume = self.current_volume(&plan.path)?;
        let next = self.recovered_binding(plan, &volume);

        match self.app_state.cas_unconfigured_locator(&next) {
            Ok(()) => Ok(self.bootstrap.inspect()),
            Err(AppStateStoreError::LocatorCasConflict { .. }) => {
                Err(ExistingHomeRecoveryError::PlanStale)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn remove_plan(&self, plan_token: &str) -> Result<(), ExistingHomeRecoveryError> {
        self.plans
            .write()
            .map_err(|_| ExistingHomeRecoveryError::StateStore("plan table poisoned".into()))?
            .remove(plan_token);
        Ok(())
    }

    fn current_volume(&self, path: &Path) -> Result<VolumeIdentity, ExistingHomeRecoveryError> {
        self.volume
            .volume_identity(path)
            .ok()
            .flatten()
            .ok_or(ExistingHomeRecoveryError::PlanStale)
    }

    fn recovered_binding(
        &self,
        plan: &ExistingHomeRecoveryPlan,
        volume: &VolumeIdentity,
    ) -> HomeBindingFile {
        HomeBindingFile {
            schema_version: HOME_BINDING_SCHEMA_VERSION,
            current: Some(HomeBindingRecord {
                home_id: plan.home_id.clone(),
                path: plan.path.clone(),
                volume_fsid: volume.fsid.clone(),
                volume_uuid: volume.uuid.clone(),
                // Recover Existing Home reconstructs the missing locator;
                // it preserves the marker/Catalog creation fact rather than
                // fabricating a new Home initialization time.
                bound_at: plan.created_at.clone(),
            }),
            abandoned: Vec::new(),
        }
    }

    fn inspect_profile(
        &self,
        selected_path: &Path,
    ) -> Result<VerifiedRecoveryProfile, ExistingHomeRecoveryError> {
        ExistingHomeRecoveryProfile::new(
            self.probe.clone(),
            self.filesystem.clone(),
            self.classifier.clone(),
            self.config.catalog_file_name.clone(),
        )
        .inspect(selected_path)
        .map_err(|error| match error {
            RecoveryProfileInspectionError::Rejected { reason } => {
                ExistingHomeRecoveryError::ProfileRejected { reason }
            }
            RecoveryProfileInspectionError::FileSystem(message) => {
                ExistingHomeRecoveryError::FileSystem(message)
            }
        })
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

    fn ensure_preparable_path(
        &self,
        selected_path: &Path,
    ) -> Result<(), ExistingHomeRecoveryError> {
        let files = self.app_state.load()?;
        self.ensure_complete_unconfigured(&files)?;
        self.ensure_snapshot_allows_path(selected_path)
    }

    fn ensure_snapshot_allows_path(
        &self,
        selected_path: &Path,
    ) -> Result<(), ExistingHomeRecoveryError> {
        match self.bootstrap.inspect() {
            BootstrapSnapshot::Unconfigured => Ok(()),
            BootstrapSnapshot::DefaultHomeRecoveryOffer { path } if path == selected_path => Ok(()),
            _ => Err(ExistingHomeRecoveryError::Ineligible {
                reason: RecoveryEligibilityRejection::BootstrapState,
            }),
        }
    }

    fn ensure_complete_unconfigured(
        &self,
        files: &AppStateFiles,
    ) -> Result<(), ExistingHomeRecoveryError> {
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
        match self.bootstrap.inspect() {
            BootstrapSnapshot::Unconfigured => {}
            BootstrapSnapshot::DefaultHomeRecoveryOffer { .. } => {}
            _ => {
                return Err(ExistingHomeRecoveryError::Ineligible {
                    reason: RecoveryEligibilityRejection::BootstrapState,
                });
            }
        }
        Ok(())
    }
}

fn token_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}
