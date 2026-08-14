//! Home Binding authority (spec §5.3, §5.4; ADR-0012 §3–§4): the one-time
//! transition from `Unconfigured` / `LegacyDetected` to a committed
//! `Bound Home`. The locator atomic commit (`home-binding.json` via the
//! tmp → fsync → rename → parent fsync protocol) is the single binding
//! commit point:
//!
//! - before the commit a crash leaves the candidate recoverable (`Continue`)
//!   or deletable (`Cancel` of only operation-created, identity-matched
//!   artifacts);
//! - after the commit a crash only rolls forward the same binding.
//!
//! Fresh candidates, the Legacy in-place transition and the Legacy copy
//! transition all funnel through this module; mixed/unknown Legacy Homes
//! stay behind the Fixture Recovery Lock before any path selection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use thiserror::Error;

use crate::core::bootstrap::{BootstrapService, BootstrapSnapshot};
use crate::core::fixture_recovery::{
    FixtureClassifier, FixtureShapeMode, epoch_seconds_to_rfc3339,
};
use crate::core::home::{HomeId, HomeMarker, VolumeIdentity};
use crate::seams::app_state_store::{
    AppStateStore, AppStateStoreError, HomeBindingFile, HomeBindingRecord, RecoveryOperationRecord,
    HOME_BINDING_SCHEMA_VERSION,
};
use crate::seams::catalog_probe::{
    CatalogHomeIdentity, CatalogProbe, CatalogProbeReport, CURRENT_CATALOG_SCHEMA_VERSION,
};
use crate::seams::filesystem::{FileSystem, FileSystemError};
use crate::seams::legacy_migration::LegacyCatalogMigrator;
use crate::seams::prepared_catalog::PreparedCatalogFactory;
use crate::seams::volume_identity::VolumeIdentitySource;

/// Active ledger operation kinds owned by this module.
pub const HOME_CANDIDATE_KIND: &str = "home_candidate";
pub const LEGACY_TRANSITION_KIND: &str = "legacy_transition";

/// Durable cursors of the binding state machine, persisted to the external
/// ledger between every write step (spec §3.2 protocol).
pub mod cursors {
    pub const PREPARING: &str = "preparing";
    pub const CREATED: &str = "created";
    pub const COPIED: &str = "copied";
    pub const VERIFIED: &str = "verified";
    pub const COMMITTED: &str = "committed";
    pub const CANCELLED: &str = "cancelled";
}

/// Minimum usable free space for a Home candidate volume (spec §5.3).
pub const MIN_HOME_SPACE_BYTES: u64 = 100 * 1024 * 1024;

/// The standard Home layout directories (spec §3.1). `cache/` is
/// rebuildable and `staging/` transient; all five exist in a fresh Home.
pub const STANDARD_LAYOUT_DIRS: [&str; 5] = ["skills", "remotes", "operations", "cache", "staging"];

/// Which one-time transition a prepared candidate carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateMode {
    /// No Legacy Home: a new Home is created at the path.
    Fresh,
    /// The Legacy Home at the default path is bound in place, zero moves.
    LegacyInPlace,
    /// The Legacy Home is copied to a custom path, then bound there.
    LegacyCopy,
}

/// Closed candidate validation failures (spec §5.3). React maps each reason
/// to a message key; `detail` holds raw technical facts for the diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateInvalidReason {
    NotAbsolute,
    NotUtf8,
    SymlinkComponent,
    StateDirOverlap,
    AgentDirOverlap,
    ParentMissing,
    ParentNotWritable,
    NotDirectory,
    NotEmpty,
    NoVolumeIdentity,
    InsufficientSpace,
    NotLegacyHome,
    LegacyContaminated,
}

/// A validated, not-yet-confirmed Home candidate (spec §4.2 `HomeCandidate`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomeCandidate {
    pub path: PathBuf,
    pub token: String,
    pub mode: CandidateMode,
    pub volume: VolumeIdentity,
    pub available_bytes: u64,
    /// The Legacy source path for a copy transition.
    pub legacy_source: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum HomeBindingError {
    #[error("Home Binding is not possible in the current bootstrap state: {0}")]
    InvalidState(String),
    #[error("another binding operation is already active: {operation_id}")]
    OperationAlreadyActive { operation_id: String },
    #[error("the candidate is invalid: {reason:?} at {path}: {detail}")]
    CandidateInvalid {
        reason: CandidateInvalidReason,
        path: PathBuf,
        detail: String,
    },
    #[error("no binding operation is active")]
    NoActiveOperation,
    #[error("no active binding operation matches {operation_id}")]
    OperationNotFound { operation_id: String },
    #[error("a SQLite writer holds the WAL index; the transition cannot safely proceed: {0}")]
    WriterActive(String),
    #[error("binding step {cursor} failed: {message}")]
    StepFailed { cursor: String, message: String },
    #[error("the binding operation state is ambiguous and cannot be converged: {0}")]
    AmbiguousState(String),
    #[error("the app state could not be read or written: {0}")]
    StateStore(String),
    #[error("filesystem operation failed: {0}")]
    Filesystem(String),
    #[error("catalog probe failed: {0}")]
    Probe(String),
    #[error("catalog migration failed: {0}")]
    Migration(String),
    #[error("not enough free space: need {required_bytes} bytes, {available_bytes} available")]
    DiskFull { required_bytes: u64, available_bytes: u64 },
    #[error("the operation cannot be cancelled: {0}")]
    NotCancellable(String),
    #[error("the candidate plan is stale; prepare the Home again")]
    PlanStale,
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<AppStateStoreError> for HomeBindingError {
    fn from(error: AppStateStoreError) -> Self {
        HomeBindingError::StateStore(error.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct HomeBindingConfig {
    pub state_dir: PathBuf,
    pub default_home_path: PathBuf,
    pub catalog_file_name: String,
    /// Known Agent skills directories (built-in Presets and the Workbench
    /// custom root); a candidate may not overlap any of them (ADR-0012 §3).
    pub agent_skill_dirs: Vec<PathBuf>,
}

#[derive(Clone)]
struct CandidatePlan {
    path: PathBuf,
    mode: CandidateMode,
}

struct ValidatedCandidate {
    path: PathBuf,
    volume: VolumeIdentity,
    available_bytes: u64,
    legacy_source: Option<PathBuf>,
}

pub struct HomeBindingService {
    app_state: Arc<dyn AppStateStore>,
    volume: Arc<dyn VolumeIdentitySource>,
    probe: Arc<dyn CatalogProbe>,
    filesystem: Arc<dyn FileSystem>,
    classifier: Arc<dyn FixtureClassifier>,
    migrator: Arc<dyn LegacyCatalogMigrator>,
    prepared: Arc<dyn PreparedCatalogFactory>,
    bootstrap: Arc<BootstrapService>,
    config: HomeBindingConfig,
    plans: RwLock<HashMap<String, CandidatePlan>>,
}

impl HomeBindingService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app_state: Arc<dyn AppStateStore>,
        volume: Arc<dyn VolumeIdentitySource>,
        probe: Arc<dyn CatalogProbe>,
        filesystem: Arc<dyn FileSystem>,
        classifier: Arc<dyn FixtureClassifier>,
        migrator: Arc<dyn LegacyCatalogMigrator>,
        prepared: Arc<dyn PreparedCatalogFactory>,
        bootstrap: Arc<BootstrapService>,
        config: HomeBindingConfig,
    ) -> Self {
        Self {
            app_state,
            volume,
            probe,
            filesystem,
            classifier,
            migrator,
            prepared,
            bootstrap,
            config,
            plans: RwLock::new(HashMap::new()),
        }
    }

    // -- public interface (spec §4.2) --------------------------------------

    /// Read-only candidate validation. The path is normalized (absolute,
    /// `~` expanded), checked for symlink components, state/Agent overlaps,
    /// emptiness, stable volume identity and free space; Legacy sources
    /// additionally pass the read-only fixture classification. Returns a
    /// token that `confirm_home` binds to this path and mode.
    pub fn prepare_home(&self, path: &Path) -> Result<HomeCandidate, HomeBindingError> {
        let snapshot = self.bootstrap.inspect();
        let legacy_path: Option<PathBuf> = match &snapshot {
            BootstrapSnapshot::Unconfigured => None,
            BootstrapSnapshot::LegacyDetected { path } => Some(path.clone()),
            BootstrapSnapshot::HomeCandidatePending { .. } => {
                return Err(HomeBindingError::InvalidState(
                    "a candidate is already pending; Continue or Cancel it first".into(),
                ));
            }
            _ => {
                return Err(HomeBindingError::InvalidState(format!(
                    "Home selection is only available from Unconfigured or LegacyDetected, not \
                     {snapshot:?}"
                )));
            }
        };
        // An empty path means "the default Home path": React never carries
        // filesystem paths, the native authority resolves the default
        // (spec §4.1).
        let requested = if path.as_os_str().is_empty() {
            self.config.default_home_path.as_path()
        } else {
            path
        };
        let normalized = self
            .filesystem
            .normalize_configured_path(requested)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        let mode = match &legacy_path {
            Some(legacy) if *legacy == normalized => CandidateMode::LegacyInPlace,
            Some(_) => CandidateMode::LegacyCopy,
            None => CandidateMode::Fresh,
        };
        let candidate = self.validate_candidate(requested, mode, legacy_path.as_deref())?;
        let token = format!("hb-{:x}", token_nanos());
        self.plans
            .write()
            .map_err(|_| HomeBindingError::Internal("plan table poisoned".into()))?
            .insert(
                token.clone(),
                CandidatePlan {
                    path: normalized.clone(),
                    mode,
                },
            );
        Ok(HomeCandidate {
            path: normalized,
            token,
            mode,
            volume: candidate.volume,
            available_bytes: candidate.available_bytes,
            legacy_source: candidate.legacy_source,
        })
    }

    /// Explicit user confirmation: re-validates the candidate, runs the
    /// transition (fresh creation, Legacy in-place migration or Legacy
    /// copy), verifies every artifact, then commits the locator — the only
    /// binding commit point. Returns the fresh bootstrap snapshot.
    pub fn confirm_home(&self, token: &str) -> Result<BootstrapSnapshot, HomeBindingError> {
        let plan = self
            .plans
            .read()
            .map_err(|_| HomeBindingError::Internal("plan table poisoned".into()))?
            .get(token)
            .cloned()
            .ok_or(HomeBindingError::PlanStale)?;
        self.confirm_plan(token, &plan)
    }

    /// Resume an interrupted binding operation from its durable cursor
    /// (crash recovery, spec §5.3/§5.4). Rolls forward deterministically;
    /// ambiguous states fail closed.
    pub fn continue_candidate(
        &self,
        operation_id: &str,
    ) -> Result<BootstrapSnapshot, HomeBindingError> {
        let files = self.app_state.load()?;
        let Some(active) = files.recovery_ledger.active.clone() else {
            return Err(HomeBindingError::NoActiveOperation);
        };
        if active.operation_id != operation_id {
            return Err(HomeBindingError::OperationNotFound {
                operation_id: operation_id.into(),
            });
        }
        match active.kind.as_str() {
            HOME_CANDIDATE_KIND => self.continue_home_candidate(&active),
            LEGACY_TRANSITION_KIND => self.continue_legacy_transition(&active),
            other => Err(HomeBindingError::AmbiguousState(format!(
                "unknown active operation kind: {other}"
            ))),
        }
    }

    /// Cancel an interrupted candidate: deletes only artifacts proven to be
    /// created by this operation (ledger identity plus pure-layout contents)
    /// and clears the ledger. A Legacy in-place transition never cancels —
    /// its Catalog holds user data, so cancellation fails closed and
    /// `Continue` remains the only path. The Legacy source of a copy
    /// transition is never touched.
    pub fn cancel_candidate(
        &self,
        operation_id: &str,
    ) -> Result<BootstrapSnapshot, HomeBindingError> {
        let files = self.app_state.load()?;
        if files.binding.current.is_some() {
            return Err(HomeBindingError::NotCancellable(
                "a binding is already committed; Continue rolls forward, cancellation is \
                 impossible"
                    .into(),
            ));
        }
        let Some(active) = files.recovery_ledger.active.clone() else {
            return Err(HomeBindingError::NoActiveOperation);
        };
        if active.operation_id != operation_id {
            return Err(HomeBindingError::OperationNotFound {
                operation_id: operation_id.into(),
            });
        }
        match active.kind.as_str() {
            HOME_CANDIDATE_KIND => {
                let path = active.live_path.clone().ok_or_else(|| {
                    HomeBindingError::AmbiguousState("the candidate has no live path".into())
                })?;
                if !self.remove_candidate_if_owned(&path, active.home_id.as_ref())? {
                    return Err(HomeBindingError::NotCancellable(
                        "the candidate directory contains content that this operation did not \
                         create; it is never deleted"
                            .into(),
                    ));
                }
                self.finish_operation(&active.operation_id, cursors::CANCELLED)?;
                Ok(self.bootstrap.inspect())
            }
            LEGACY_TRANSITION_KIND => {
                let destination = active.prepared_path.clone().ok_or_else(|| {
                    HomeBindingError::AmbiguousState(
                        "the copy transition has no destination path".into(),
                    )
                })?;
                // The destination was validated empty/nonexistent before the
                // operation started; the ledger proves every byte there came
                // from this operation's copy. The Legacy source is untouched.
                self.remove_tree_if_present(&destination)?;
                self.finish_operation(&active.operation_id, cursors::CANCELLED)?;
                Ok(self.bootstrap.inspect())
            }
            other => Err(HomeBindingError::AmbiguousState(format!(
                "unknown active operation kind: {other}"
            ))),
        }
    }

    // -- validation --------------------------------------------------------

    fn validate_candidate(
        &self,
        requested: &Path,
        mode: CandidateMode,
        legacy: Option<&Path>,
    ) -> Result<ValidatedCandidate, HomeBindingError> {
        // Symlink components must be checked on the raw path: normalization
        // canonicalizes existing ancestors and would erase them.
        if !self
            .filesystem
            .path_has_no_symlink_component(requested)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?
        {
            return Err(self.invalid(CandidateInvalidReason::SymlinkComponent, requested));
        }
        let normalized = self
            .filesystem
            .normalize_configured_path(requested)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        if !normalized.is_absolute() {
            return Err(self.invalid(CandidateInvalidReason::NotAbsolute, &normalized));
        }
        if normalized.to_str().is_none() {
            return Err(self.invalid(CandidateInvalidReason::NotUtf8, &normalized));
        }
        if paths_overlap(&normalized, &self.config.state_dir) {
            return Err(self.invalid(
                CandidateInvalidReason::StateDirOverlap,
                &normalized,
            ));
        }
        for agent_dir in &self.config.agent_skill_dirs {
            if paths_overlap(&normalized, agent_dir) {
                return Err(self.invalid(
                    CandidateInvalidReason::AgentDirOverlap,
                    &normalized,
                ));
            }
        }

        let mut exists = false;
        let mut non_empty = false;
        match self.filesystem.list_directory(&normalized) {
            Ok(entries) => {
                exists = true;
                non_empty = !entries.is_empty();
            }
            Err(error) if is_not_found(&error) => {}
            Err(_) => {
                return Err(self.invalid(
                    CandidateInvalidReason::NotDirectory,
                    &normalized,
                ));
            }
        }
        if exists {
            // A Legacy in-place transition targets an existing Home by
            // definition; only fresh/copy destinations must be empty.
            if non_empty && mode != CandidateMode::LegacyInPlace {
                return Err(self.invalid(CandidateInvalidReason::NotEmpty, &normalized));
            }
        } else {
            let Some(parent) = normalized.parent() else {
                return Err(self.invalid(CandidateInvalidReason::ParentMissing, &normalized));
            };
            if !self
                .filesystem
                .path_is_directory(&parent)
                .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?
            {
                return Err(self.invalid(CandidateInvalidReason::ParentMissing, &normalized));
            }
            if !self
                .filesystem
                .path_is_writable(&parent)
                .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?
            {
                return Err(self.invalid(
                    CandidateInvalidReason::ParentNotWritable,
                    &normalized,
                ));
            }
        }

        let volume = match self
            .volume
            .volume_identity(&normalized)
            .map_err(|error| HomeBindingError::CandidateInvalid {
                reason: CandidateInvalidReason::NoVolumeIdentity,
                path: normalized.clone(),
                detail: error.to_string(),
            })?
        {
            Some(volume) => volume,
            None => {
                return Err(self.invalid(
                    CandidateInvalidReason::NoVolumeIdentity,
                    &normalized,
                ));
            }
        };
        let available = self
            .filesystem
            .available_space(&normalized)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;

        match mode {
            CandidateMode::Fresh => {
                if available < MIN_HOME_SPACE_BYTES {
                    return Err(HomeBindingError::DiskFull {
                        required_bytes: MIN_HOME_SPACE_BYTES,
                        available_bytes: available,
                    });
                }
                Ok(ValidatedCandidate {
                    path: normalized.clone(),
                    volume,
                    available_bytes: available,
                    legacy_source: None,
                })
            }
            CandidateMode::LegacyInPlace => {
                self.verify_legacy_source(&normalized)?;
                Ok(ValidatedCandidate {
                    path: normalized.clone(),
                    volume,
                    available_bytes: available,
                    legacy_source: None,
                })
            }
            CandidateMode::LegacyCopy => {
                let source = legacy
                    .ok_or_else(|| {
                        HomeBindingError::InvalidState(
                            "the Legacy copy transition has no source path".into(),
                        )
                    })?
                    .to_path_buf();
                self.verify_legacy_source(&source)?;
                let source_size = self
                    .filesystem
                    .tree_size(&source)
                    .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
                let required = source_size.max(MIN_HOME_SPACE_BYTES);
                if available < required {
                    return Err(HomeBindingError::DiskFull {
                        required_bytes: required,
                        available_bytes: available,
                    });
                }
                Ok(ValidatedCandidate {
                    path: normalized.clone(),
                    volume,
                    available_bytes: available,
                    legacy_source: Some(source),
                })
            }
        }
    }

    fn verify_legacy_source(&self, path: &Path) -> Result<(), HomeBindingError> {
        let catalog = path.join(&self.config.catalog_file_name);
        let report = self.probe.probe(&catalog).map_err(|error| {
            HomeBindingError::Probe(format!("could not probe the Legacy Catalog: {error}"))
        })?;
        let is_legacy = report
            .schema_version
            .is_some_and(|version| version > 0 && version < CURRENT_CATALOG_SCHEMA_VERSION);
        // A fixture-recovery prepared Home is a current-schema Catalog
        // without identity; it binds in place without a migration.
        let is_prepared = report.schema_version == Some(CURRENT_CATALOG_SCHEMA_VERSION)
            && report.home_identity.is_none();
        if !report.exists || !(is_legacy || is_prepared) {
            return Err(HomeBindingError::CandidateInvalid {
                reason: CandidateInvalidReason::NotLegacyHome,
                path: path.to_path_buf(),
                detail: format!(
                    "{} is not a bindable Legacy or prepared Home Catalog",
                    catalog.display()
                ),
            });
        }
        let classification = self.classifier.classify(path, FixtureShapeMode::Legacy);
        if classification.is_contaminated() {
            return Err(HomeBindingError::CandidateInvalid {
                reason: CandidateInvalidReason::LegacyContaminated,
                path: path.to_path_buf(),
                detail: format!("Legacy classification: {classification:?}"),
            });
        }
        Ok(())
    }

    fn invalid(&self, reason: CandidateInvalidReason, path: &Path) -> HomeBindingError {
        HomeBindingError::CandidateInvalid {
            reason,
            path: path.to_path_buf(),
            detail: format!("candidate path: {}", path.display()),
        }
    }

    // -- confirmation ------------------------------------------------------

    fn confirm_plan(
        &self,
        token: &str,
        plan: &CandidatePlan,
    ) -> Result<BootstrapSnapshot, HomeBindingError> {
        let snapshot = self.bootstrap.inspect();
        match &snapshot {
            BootstrapSnapshot::Unconfigured => {
                if plan.mode != CandidateMode::Fresh {
                    return Err(HomeBindingError::InvalidState(
                        "a Legacy Home is present; a fresh candidate cannot be confirmed".into(),
                    ));
                }
            }
            BootstrapSnapshot::LegacyDetected { path } => {
                if plan.mode == CandidateMode::Fresh {
                    return Err(HomeBindingError::InvalidState(
                        "a Legacy Home is present; only the Legacy transition can be confirmed"
                            .into(),
                    ));
                }
                if plan.mode == CandidateMode::LegacyInPlace && *path != plan.path {
                    return Err(HomeBindingError::InvalidState(
                        "the Legacy path changed since the candidate was prepared".into(),
                    ));
                }
            }
            _ => {
                return Err(HomeBindingError::InvalidState(
                    "Home Binding is only confirmable from Unconfigured or LegacyDetected".into(),
                ));
            }
        }
        let files = self.app_state.load()?;
        if let Some(active) = &files.recovery_ledger.active {
            return Err(HomeBindingError::OperationAlreadyActive {
                operation_id: active.operation_id.clone(),
            });
        }
        let legacy = match (&snapshot, plan.mode) {
            (BootstrapSnapshot::LegacyDetected { path }, _) => Some(path.clone()),
            _ => None,
        };
        let candidate = self.validate_candidate(&plan.path, plan.mode, legacy.as_deref())?;
        let result = match plan.mode {
            CandidateMode::Fresh => self.confirm_fresh(candidate),
            CandidateMode::LegacyInPlace => self.confirm_legacy_in_place(candidate),
            CandidateMode::LegacyCopy => self.confirm_legacy_copy(candidate),
        };
        if result.is_ok() {
            if let Ok(mut plans) = self.plans.write() {
                plans.remove(token);
            }
        }
        result
    }

    fn confirm_fresh(
        &self,
        candidate: ValidatedCandidate,
    ) -> Result<BootstrapSnapshot, HomeBindingError> {
        let home_id = new_home_id();
        let bound_at = rfc3339_now();
        let operation_id = new_operation_id();
        self.start_operation(
            &operation_id,
            HOME_CANDIDATE_KIND,
            Some(&home_id),
            Some(&candidate.path),
            None,
            &bound_at,
        )?;
        self.ensure_candidate_created(
            &operation_id,
            &home_id,
            &candidate.path,
            &candidate.volume,
            &bound_at,
        )?;
        self.step_verify_candidate(
            &operation_id,
            &home_id,
            &candidate.path,
            &candidate.volume,
            &bound_at,
        )?;
        self.commit_locator(&home_id, &candidate.path, &candidate.volume, &bound_at)?;
        self.finish_operation(&operation_id, cursors::COMMITTED)?;
        Ok(self.bootstrap.inspect())
    }

    fn confirm_legacy_in_place(
        &self,
        candidate: ValidatedCandidate,
    ) -> Result<BootstrapSnapshot, HomeBindingError> {
        let home_id = new_home_id();
        let bound_at = rfc3339_now();
        let operation_id = new_operation_id();
        self.start_operation(
            &operation_id,
            HOME_CANDIDATE_KIND,
            Some(&home_id),
            Some(&candidate.path),
            None,
            &bound_at,
        )?;
        self.quiesce(&candidate.path)?;
        self.migrate_catalog_with_identity(&candidate.path, &home_id, &candidate.volume, &bound_at)?;
        self.write_marker(&candidate.path, &home_id, &candidate.volume, &bound_at)?;
        self.ensure_layout(&candidate.path)?;
        self.step_verify_candidate(
            &operation_id,
            &home_id,
            &candidate.path,
            &candidate.volume,
            &bound_at,
        )?;
        self.commit_locator(&home_id, &candidate.path, &candidate.volume, &bound_at)?;
        self.finish_operation(&operation_id, cursors::COMMITTED)?;
        Ok(self.bootstrap.inspect())
    }

    fn confirm_legacy_copy(
        &self,
        candidate: ValidatedCandidate,
    ) -> Result<BootstrapSnapshot, HomeBindingError> {
        let source = candidate.legacy_source.clone().ok_or_else(|| {
            HomeBindingError::InvalidState("the copy transition has no source path".into())
        })?;
        let home_id = new_home_id();
        let bound_at = rfc3339_now();
        let operation_id = new_operation_id();
        self.start_operation(
            &operation_id,
            LEGACY_TRANSITION_KIND,
            Some(&home_id),
            Some(&source),
            Some(&candidate.path),
            &bound_at,
        )?;
        self.quiesce(&source)?;
        self.migrator
            .checkpoint(&source.join(&self.config.catalog_file_name))
            .map_err(|error| HomeBindingError::Migration(error.to_string()))?;
        self.copy_legacy_tree(&operation_id, &source, &candidate.path)?;
        self.finalize_copy_target(
            &operation_id,
            &home_id,
            &candidate.path,
            &candidate.volume,
            &bound_at,
        )?;
        self.commit_locator(&home_id, &candidate.path, &candidate.volume, &bound_at)?;
        self.finish_operation(&operation_id, cursors::COMMITTED)?;
        Ok(self.bootstrap.inspect())
    }

    // -- crash convergence -------------------------------------------------

    fn continue_home_candidate(
        &self,
        op: &RecoveryOperationRecord,
    ) -> Result<BootstrapSnapshot, HomeBindingError> {
        let path = op.live_path.clone().ok_or_else(|| {
            HomeBindingError::AmbiguousState("the candidate has no live path".into())
        })?;
        let home_id = op.home_id.clone().ok_or_else(|| {
            HomeBindingError::AmbiguousState("the candidate has no Home identity".into())
        })?;
        let bound_at = op.created_at.clone();
        let volume = self.require_volume(&path)?;
        match op.cursor.as_deref() {
            None | Some(cursors::PREPARING) | Some(cursors::CREATED) | Some(cursors::VERIFIED) => {
                self.ensure_candidate_complete(&op.operation_id, &home_id, &path, &volume, &bound_at)?;
            }
            Some(cursors::COMMITTED) => {
                self.finish_operation(&op.operation_id, cursors::COMMITTED)?;
                return Ok(self.bootstrap.inspect());
            }
            other => {
                return Err(HomeBindingError::AmbiguousState(format!(
                    "unknown candidate cursor {other:?}"
                )));
            }
        }
        self.commit_locator(&home_id, &path, &volume, &bound_at)?;
        self.finish_operation(&op.operation_id, cursors::COMMITTED)?;
        Ok(self.bootstrap.inspect())
    }

    fn continue_legacy_transition(
        &self,
        op: &RecoveryOperationRecord,
    ) -> Result<BootstrapSnapshot, HomeBindingError> {
        let source = op.live_path.clone().ok_or_else(|| {
            HomeBindingError::AmbiguousState("the copy transition has no source path".into())
        })?;
        let destination = op.prepared_path.clone().ok_or_else(|| {
            HomeBindingError::AmbiguousState("the copy transition has no destination path".into())
        })?;
        let home_id = op.home_id.clone().ok_or_else(|| {
            HomeBindingError::AmbiguousState("the copy transition has no Home identity".into())
        })?;
        let bound_at = op.created_at.clone();
        let volume = self.require_volume(&destination)?;
        match op.cursor.as_deref() {
            None | Some(cursors::PREPARING) | Some(cursors::COPIED) => {
                self.quiesce(&source)?;
                self.migrator
                    .checkpoint(&source.join(&self.config.catalog_file_name))
                    .map_err(|error| HomeBindingError::Migration(error.to_string()))?;
                self.copy_legacy_tree(&op.operation_id, &source, &destination)?;
                self.finalize_copy_target(
                    &op.operation_id,
                    &home_id,
                    &destination,
                    &volume,
                    &bound_at,
                )?;
            }
            Some(cursors::VERIFIED) => {}
            Some(cursors::COMMITTED) => {
                self.finish_operation(&op.operation_id, cursors::COMMITTED)?;
                return Ok(self.bootstrap.inspect());
            }
            other => {
                return Err(HomeBindingError::AmbiguousState(format!(
                    "unknown copy-transition cursor {other:?}"
                )));
            }
        }
        self.commit_locator(&home_id, &destination, &volume, &bound_at)?;
        self.finish_operation(&op.operation_id, cursors::COMMITTED)?;
        Ok(self.bootstrap.inspect())
    }

    /// Converge a home_candidate operation to the verified state no matter
    /// which step it crashed at: creates missing artifacts, completes a
    /// half-done in-place migration, writes the marker, then verifies.
    fn ensure_candidate_complete(
        &self,
        operation_id: &str,
        home_id: &HomeId,
        path: &Path,
        volume: &VolumeIdentity,
        bound_at: &str,
    ) -> Result<(), HomeBindingError> {
        self.ensure_candidate_created(operation_id, home_id, path, volume, bound_at)?;
        self.step_verify_candidate(operation_id, home_id, path, volume, bound_at)
    }

    /// Make the candidate artifacts exist and converge to the `created`
    /// state no matter which step a crash interrupted: a missing Catalog is
    /// created (directory reused when empty), a pre-identity Catalog is
    /// migrated, the marker is written when missing and the standard layout
    /// is ensured.
    fn ensure_candidate_created(
        &self,
        operation_id: &str,
        home_id: &HomeId,
        path: &Path,
        volume: &VolumeIdentity,
        bound_at: &str,
    ) -> Result<(), HomeBindingError> {
        let catalog_path = path.join(&self.config.catalog_file_name);
        let report = self.probe.probe(&catalog_path).map_err(|error| {
            HomeBindingError::StepFailed {
                cursor: cursors::PREPARING.into(),
                message: format!("could not probe the candidate Catalog: {error}"),
            }
        })?;
        if report.exists {
            // An in-place transition may have migrated the Catalog already.
            self.ensure_catalog_identity(&catalog_path, report, home_id, volume, bound_at)?;
        } else {
            // Fresh creation: the directory must be missing or empty, then
            // the standard layout, Catalog and marker are created.
            match self.filesystem.list_directory(path) {
                Ok(_) => {}
                Err(error) if is_not_found(&error) => {
                    self.filesystem
                        .create_directory_all(path)
                        .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
                }
                Err(_) => {
                    return Err(HomeBindingError::StepFailed {
                        cursor: cursors::PREPARING.into(),
                        message: "the candidate path is not an empty directory".into(),
                    });
                }
            }
            self.ensure_layout(path)?;
            self.prepared
                .create_prepared(
                    &catalog_path,
                    Some(&CatalogHomeIdentity {
                        home_id: home_id.clone(),
                        volume_fsid: volume.fsid.clone(),
                        volume_uuid: volume.uuid.clone(),
                        home_bound_at: bound_at.to_string(),
                    }),
                )
                .map_err(|error| HomeBindingError::StepFailed {
                    cursor: cursors::PREPARING.into(),
                    message: format!("could not create the candidate Catalog: {error}"),
                })?;
            self.write_marker(path, home_id, volume, bound_at)?;
            self.filesystem
                .fsync_directory(path)
                .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
            self.set_cursor(operation_id, cursors::CREATED)?;
        }
        // The marker may be missing after a crash between the migration and
        // the marker write; the layout may lack the vNext directories.
        let marker_missing = self
            .filesystem
            .read_utf8_file(&path.join(HomeMarker::FILE_NAME))
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?
            .and_then(|content| HomeMarker::parse(&content))
            .is_none();
        if marker_missing {
            self.write_marker(path, home_id, volume, bound_at)?;
        }
        self.ensure_layout(path)?;
        Ok(())
    }

    /// Migrate a pre-identity Catalog (or verify it already carries the
    /// binding identity after an earlier crash).
    fn ensure_catalog_identity(
        &self,
        catalog_path: &Path,
        report: CatalogProbeReport,
        home_id: &HomeId,
        volume: &VolumeIdentity,
        bound_at: &str,
    ) -> Result<(), HomeBindingError> {
        match report.schema_version {
            Some(version) if version < CURRENT_CATALOG_SCHEMA_VERSION => {
                self.migrator
                    .migrate_with_identity(
                        catalog_path,
                        &CatalogHomeIdentity {
                            home_id: home_id.clone(),
                            volume_fsid: volume.fsid.clone(),
                            volume_uuid: volume.uuid.clone(),
                            home_bound_at: bound_at.to_string(),
                        },
                    )
                    .map_err(|error| HomeBindingError::Migration(error.to_string()))?;
                Ok(())
            }
            Some(CURRENT_CATALOG_SCHEMA_VERSION) => {
                let identity = report.home_identity.ok_or_else(|| {
                    HomeBindingError::AmbiguousState(
                        "the candidate Catalog has no Home identity".into(),
                    )
                })?;
                if identity.home_id != *home_id
                    || identity.volume_fsid != volume.fsid
                    || identity.volume_uuid != volume.uuid
                {
                    return Err(HomeBindingError::AmbiguousState(
                        "the candidate Catalog identity does not match the operation".into(),
                    ));
                }
                Ok(())
            }
            _ => Err(HomeBindingError::AmbiguousState(
                "the candidate Catalog schema is unreadable".into(),
            )),
        }
    }

    fn finalize_copy_target(
        &self,
        operation_id: &str,
        home_id: &HomeId,
        destination: &Path,
        volume: &VolumeIdentity,
        bound_at: &str,
    ) -> Result<(), HomeBindingError> {
        let catalog_path = destination.join(&self.config.catalog_file_name);
        let report = self.probe.probe(&catalog_path).map_err(|error| {
            HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: format!("could not probe the copied Catalog: {error}"),
            }
        })?;
        if !report.exists {
            return Err(HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: "the copied Catalog is missing".into(),
            });
        }
        self.ensure_catalog_identity(&catalog_path, report, home_id, volume, bound_at)?;
        self.write_marker(destination, home_id, volume, bound_at)?;
        self.ensure_layout(destination)?;
        self.step_verify_candidate(operation_id, home_id, destination, volume, bound_at)?;
        Ok(())
    }

    fn copy_legacy_tree(
        &self,
        operation_id: &str,
        source: &Path,
        destination: &Path,
    ) -> Result<(), HomeBindingError> {
        if self.legacy_copy_matches(source, destination)? {
            // The interrupted copy already produced a consistent tree.
            self.set_cursor(operation_id, cursors::COPIED)?;
            return Ok(());
        }
        // The destination holds a partial or stale copy: it was validated
        // empty/nonexistent before this operation started, so every byte
        // there came from this operation. Remove and copy again.
        self.remove_tree_if_present(destination)?;
        self.filesystem
            .copy_tree_verified(source, destination)
            .map_err(|error| HomeBindingError::StepFailed {
                cursor: cursors::COPIED.into(),
                message: format!("could not copy the Legacy Home: {error}"),
            })?;
        if !self.legacy_copy_matches(source, destination)? {
            return Err(HomeBindingError::StepFailed {
                cursor: cursors::COPIED.into(),
                message: "the copied tree does not match the source after copy".into(),
            });
        }
        self.set_cursor(operation_id, cursors::COPIED)?;
        Ok(())
    }

    /// Per-file size plus tree hash equality between the source and the
    /// copied destination (SQLite WAL/SHM sidecars excluded: they are
    /// derived artifacts of a consistent set, never compared).
    fn legacy_copy_matches(&self, source: &Path, destination: &Path) -> Result<bool, HomeBindingError> {
        let exists = match self.filesystem.path_is_directory(destination) {
            Ok(exists) => exists,
            Err(_) => return Ok(false),
        };
        if !exists {
            return Ok(false);
        }
        let excluded = [
            format!("{}-wal", self.config.catalog_file_name),
            format!("{}-shm", self.config.catalog_file_name),
        ];
        let source_hash = self
            .filesystem
            .tree_hash_excluding(source, &excluded)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        let destination_hash = self
            .filesystem
            .tree_hash_excluding(destination, &excluded)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        if source_hash != destination_hash {
            return Ok(false);
        }
        let source_size = self
            .filesystem
            .tree_size(source)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        let destination_size = self
            .filesystem
            .tree_size(destination)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        Ok(source_size == destination_size)
    }

    // -- shared steps ------------------------------------------------------

    fn start_operation(
        &self,
        operation_id: &str,
        kind: &str,
        home_id: Option<&HomeId>,
        live_path: Option<&Path>,
        prepared_path: Option<&Path>,
        created_at: &str,
    ) -> Result<(), HomeBindingError> {
        let mut ledger = self.app_state.load()?.recovery_ledger;
        if ledger.active.is_some() {
            return Err(HomeBindingError::OperationAlreadyActive {
                operation_id: ledger
                    .active
                    .as_ref()
                    .map(|record| record.operation_id.clone())
                    .unwrap_or_default(),
            });
        }
        ledger.active = Some(RecoveryOperationRecord {
            operation_id: operation_id.into(),
            kind: kind.into(),
            home_id: home_id.cloned(),
            live_path: live_path.map(Path::to_path_buf),
            prepared_path: prepared_path.map(Path::to_path_buf),
            snapshot_path: None,
            manifest_hash: None,
            external_probe: None,
            cursor: Some(cursors::PREPARING.into()),
            commit_point: None,
            created_at: created_at.into(),
        });
        self.app_state.write_recovery_ledger(&ledger)?;
        Ok(())
    }

    fn set_cursor(&self, operation_id: &str, cursor: &str) -> Result<(), HomeBindingError> {
        let mut ledger = self.app_state.load()?.recovery_ledger;
        let Some(active) = &mut ledger.active else {
            return Err(HomeBindingError::NoActiveOperation);
        };
        if active.operation_id != operation_id {
            return Err(HomeBindingError::OperationNotFound {
                operation_id: operation_id.into(),
            });
        }
        active.cursor = Some(cursor.into());
        self.app_state.write_recovery_ledger(&ledger)?;
        Ok(())
    }

    fn finish_operation(&self, operation_id: &str, cursor: &str) -> Result<(), HomeBindingError> {
        let mut ledger = self.app_state.load()?.recovery_ledger;
        let Some(mut active) = ledger.active.take() else {
            return Err(HomeBindingError::NoActiveOperation);
        };
        if active.operation_id != operation_id {
            return Err(HomeBindingError::OperationNotFound {
                operation_id: operation_id.into(),
            });
        }
        active.cursor = Some(cursor.into());
        active.commit_point = Some(cursor.into());
        ledger.completed.push(active);
        self.app_state.write_recovery_ledger(&ledger)?;
        Ok(())
    }

    fn step_verify_candidate(
        &self,
        operation_id: &str,
        home_id: &HomeId,
        path: &Path,
        volume: &VolumeIdentity,
        bound_at: &str,
    ) -> Result<(), HomeBindingError> {
        let catalog_path = path.join(&self.config.catalog_file_name);
        let report = self.probe.probe(&catalog_path).map_err(|error| {
            HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: format!("could not probe the candidate Catalog: {error}"),
            }
        })?;
        if !report.exists {
            return Err(HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: "the candidate Catalog does not exist".into(),
            });
        }
        if report.schema_version != Some(CURRENT_CATALOG_SCHEMA_VERSION) {
            return Err(HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: format!(
                    "the candidate Catalog schema is {:?}, expected v{CURRENT_CATALOG_SCHEMA_VERSION}",
                    report.schema_version
                ),
            });
        }
        if !report.integrity_ok || !report.foreign_keys_ok {
            return Err(HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: "the candidate Catalog failed integrity or foreign-key verification"
                    .into(),
            });
        }
        let Some(identity) = &report.home_identity else {
            return Err(HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: "the candidate Catalog has no Home identity".into(),
            });
        };
        if identity.home_id != *home_id
            || identity.volume_fsid != volume.fsid
            || identity.volume_uuid != volume.uuid
        {
            return Err(HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: "the candidate Catalog identity does not match the binding".into(),
            });
        }
        let marker_path = path.join(HomeMarker::FILE_NAME);
        let marker = self
            .filesystem
            .read_utf8_file(&marker_path)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?
            .and_then(|content| HomeMarker::parse(&content))
            .ok_or_else(|| HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: "the Home marker is missing or invalid".into(),
            })?;
        if marker.home_id != *home_id
            || marker.volume_fsid != volume.fsid
            || marker.volume_uuid != volume.uuid
            || marker.created_at != bound_at
        {
            return Err(HomeBindingError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message: "the Home marker does not match the binding identity".into(),
            });
        }
        for directory in STANDARD_LAYOUT_DIRS {
            if !self
                .filesystem
                .path_is_directory(&path.join(directory))
                .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?
            {
                return Err(HomeBindingError::StepFailed {
                    cursor: cursors::VERIFIED.into(),
                    message: format!("the standard layout directory {directory} is missing"),
                });
            }
        }
        self.set_cursor(operation_id, cursors::VERIFIED)?;
        Ok(())
    }

    /// The locator atomic write is the single binding commit point
    /// (spec §5.3). Idempotent for the same binding so a post-commit crash
    /// only rolls forward; a different committed binding is a hard error.
    fn commit_locator(
        &self,
        home_id: &HomeId,
        path: &Path,
        volume: &VolumeIdentity,
        bound_at: &str,
    ) -> Result<(), HomeBindingError> {
        let files = self.app_state.load()?;
        if let Some(current) = &files.binding.current {
            if current.home_id == *home_id && current.path == *path {
                return Ok(());
            }
            return Err(HomeBindingError::InvalidState(
                "a different binding is already committed".into(),
            ));
        }
        let binding = HomeBindingFile {
            schema_version: HOME_BINDING_SCHEMA_VERSION,
            current: Some(HomeBindingRecord {
                home_id: home_id.clone(),
                path: path.to_path_buf(),
                volume_fsid: volume.fsid.clone(),
                volume_uuid: volume.uuid.clone(),
                bound_at: bound_at.into(),
            }),
            abandoned: files.binding.abandoned,
        };
        self.app_state.write_locator(&binding)?;
        Ok(())
    }

    fn write_marker(
        &self,
        path: &Path,
        home_id: &HomeId,
        volume: &VolumeIdentity,
        bound_at: &str,
    ) -> Result<(), HomeBindingError> {
        let marker = HomeMarker {
            schema_version: HomeMarker::SCHEMA_VERSION,
            home_id: home_id.clone(),
            volume_fsid: volume.fsid.clone(),
            volume_uuid: volume.uuid.clone(),
            created_at: bound_at.into(),
        };
        let json = serde_json::to_string_pretty(&marker)
            .map_err(|error| HomeBindingError::Internal(error.to_string()))?;
        self.filesystem
            .write_utf8_file(&path.join(HomeMarker::FILE_NAME), &json)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        Ok(())
    }

    fn ensure_layout(&self, path: &Path) -> Result<(), HomeBindingError> {
        for directory in STANDARD_LAYOUT_DIRS {
            self.filesystem
                .ensure_directory(&path.join(directory))
                .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        }
        Ok(())
    }

    fn migrate_catalog_with_identity(
        &self,
        path: &Path,
        home_id: &HomeId,
        volume: &VolumeIdentity,
        bound_at: &str,
    ) -> Result<(), HomeBindingError> {
        self.migrator
            .migrate_with_identity(
                &path.join(&self.config.catalog_file_name),
                &CatalogHomeIdentity {
                    home_id: home_id.clone(),
                    volume_fsid: volume.fsid.clone(),
                    volume_uuid: volume.uuid.clone(),
                    home_bound_at: bound_at.into(),
                },
            )
            .map_err(|error| HomeBindingError::Migration(error.to_string()))?;
        Ok(())
    }

    fn quiesce(&self, home_path: &Path) -> Result<(), HomeBindingError> {
        let shm_path = PathBuf::from(format!(
            "{}-shm",
            home_path.join(&self.config.catalog_file_name).display()
        ));
        match self
            .filesystem
            .try_lock_wal_index_exclusive(&shm_path)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?
        {
            true => Ok(()),
            false => Err(HomeBindingError::WriterActive(
                "the SQLite WAL index is locked by another process".into(),
            )),
        }
    }

    fn require_volume(&self, path: &Path) -> Result<VolumeIdentity, HomeBindingError> {
        match self
            .volume
            .volume_identity(path)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?
        {
            Some(volume) => Ok(volume),
            None => Err(HomeBindingError::CandidateInvalid {
                reason: CandidateInvalidReason::NoVolumeIdentity,
                path: path.to_path_buf(),
                detail: format!("no stable volume identity for {}", path.display()),
            }),
        }
    }

    fn remove_tree_if_present(&self, path: &Path) -> Result<(), HomeBindingError> {
        match self.filesystem.path_is_directory(path) {
            Ok(true) => {
                self.filesystem
                    .remove_directory_verified(path)
                    .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
                if let Some(parent) = path.parent() {
                    self.filesystem
                        .fsync_directory(parent)
                        .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
                }
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(_) => Ok(()),
        }
    }

    /// Fail-closed ownership proof for cancellation: every top-level entry
    /// must be one of the standard artifacts, and any recorded identity
    /// (marker or Catalog) must match the operation's `home_id`. A Catalog
    /// with real Skill/Agent rows (a migrated Legacy Home) is never deleted.
    fn remove_candidate_if_owned(
        &self,
        path: &Path,
        expected: Option<&HomeId>,
    ) -> Result<bool, HomeBindingError> {
        let entries = match self.filesystem.list_directory(path) {
            Ok(entries) => entries,
            Err(error) if is_not_found(&error) => return Ok(true),
            Err(_) => {
                return Err(HomeBindingError::NotCancellable(
                    "the candidate path is not a directory".into(),
                ));
            }
        };
        let catalog = &self.config.catalog_file_name;
        let allowed: Vec<String> = STANDARD_LAYOUT_DIRS
            .iter()
            .map(|directory| directory.to_string())
            .chain([
                catalog.clone(),
                format!("{catalog}-wal"),
                format!("{catalog}-shm"),
                HomeMarker::FILE_NAME.into(),
            ])
            .collect();
        for entry in &entries {
            if !allowed.contains(&entry.name) {
                return Ok(false);
            }
        }
        let catalog_path = path.join(catalog);
        let catalog_report = self.probe.probe(&catalog_path).ok();
        let catalog_exists = catalog_report
            .as_ref()
            .map(|report| report.exists)
            .unwrap_or(false);
        if let Some(expected) = expected {
            let marker_content = self
                .filesystem
                .read_utf8_file(&path.join(HomeMarker::FILE_NAME))
                .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
            match marker_content.and_then(|content| HomeMarker::parse(&content)) {
                Some(marker) => {
                    if &marker.home_id != expected {
                        return Ok(false);
                    }
                }
                None => {
                    // No valid marker: the Catalog must carry the identity
                    // (or not exist yet) for the directory to be ours.
                    match catalog_report.as_ref() {
                        Some(report) if report.exists => {
                            match &report.home_identity {
                                Some(identity) if &identity.home_id == expected => {}
                                _ => return Ok(false),
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // A Catalog that holds real Skill/Agent rows belongs to a migrated
        // Legacy Home; it is never deleted. A fresh candidate's Catalog is
        // always empty (schema + preferences only).
        if catalog_exists {
            let evidence = self.probe.probe_fixture(&catalog_path).map_err(|error| {
                HomeBindingError::Probe(format!(
                    "could not probe the candidate Catalog: {error}"
                ))
            })?;
            if !evidence.skills.is_empty() || !evidence.agents.is_empty() {
                return Ok(false);
            }
        }
        self.filesystem
            .remove_directory_verified(path)
            .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        if let Some(parent) = path.parent() {
            self.filesystem
                .fsync_directory(parent)
                .map_err(|error| HomeBindingError::Filesystem(error.to_string()))?;
        }
        Ok(true)
    }
}

// -- helpers ---------------------------------------------------------------

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn is_not_found(error: &FileSystemError) -> bool {
    matches!(
        error,
        FileSystemError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound
    )
}

fn token_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn new_operation_id() -> String {
    format!("hb-{:x}", token_nanos())
}

fn rfc3339_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    epoch_seconds_to_rfc3339(seconds)
}

/// UUID v4 from the OS entropy source. Falls back to a time-seeded value on
/// exotic failures; the shape is always a valid v4 identifier.
fn new_home_id() -> HomeId {
    let mut bytes = [0u8; 16];
    let mut seeded = false;
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if file.read_exact(&mut bytes).is_ok() {
            seeded = true;
        }
    }
    if !seeded {
        let nanos = token_nanos() as u64;
        bytes[..8].copy_from_slice(&nanos.to_be_bytes());
        bytes[8..].copy_from_slice(&(nanos ^ 0x9e37_79b9_7f4a_7c15).to_be_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    HomeId(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}
