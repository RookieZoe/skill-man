//! Shared, read-only Recovery Profile inspection for Existing Home Recovery
//! and the default-path bootstrap offer. The profile owns no App-state or
//! plan token: callers decide eligibility and commit policy separately.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::fixture_recovery::{FixtureClassifier, FixtureShapeMode};
use crate::core::home::{HomeId, HomeMarker};
use crate::core::home_binding::STANDARD_LAYOUT_DIRS;
use crate::seams::catalog_probe::{CatalogProbe, CatalogProbeError};
use crate::seams::filesystem::{FileSystem, FileSystemError};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryProfileInspectionError {
    Rejected { reason: RecoveryProfileRejection },
    FileSystem(String),
}

/// The non-secret facts recovered from the marker and Catalog after a
/// complete Recovery Profile succeeds. These are deliberately internal to
/// Core; only the service turns them into a user-confirmable plan.
#[derive(Clone, Debug)]
pub struct VerifiedRecoveryProfile {
    pub path: PathBuf,
    pub marker_home_id: HomeId,
    pub marker_created_at: String,
}

/// One implementation of the complete Existing Home Recovery Profile. It is
/// read-only and safe for Bootstrap to call before any Home is bound.
pub struct ExistingHomeRecoveryProfile {
    probe: Arc<dyn CatalogProbe>,
    filesystem: Arc<dyn FileSystem>,
    classifier: Arc<dyn FixtureClassifier>,
    catalog_file_name: String,
}

impl ExistingHomeRecoveryProfile {
    pub fn new(
        probe: Arc<dyn CatalogProbe>,
        filesystem: Arc<dyn FileSystem>,
        classifier: Arc<dyn FixtureClassifier>,
        catalog_file_name: String,
    ) -> Self {
        Self {
            probe,
            filesystem,
            classifier,
            catalog_file_name,
        }
    }

    pub fn inspect(
        &self,
        selected_path: &Path,
    ) -> Result<VerifiedRecoveryProfile, RecoveryProfileInspectionError> {
        let path = match self.filesystem.canonical_directory(selected_path) {
            Ok(path) => path,
            Err(FileSystemError::NotDirectory { .. }) => {
                return Err(rejected(RecoveryProfileRejection::NotDirectory));
            }
            Err(error) => {
                return Err(RecoveryProfileInspectionError::FileSystem(
                    error.to_string(),
                ));
            }
        };
        let marker = self.read_marker(&path)?;
        if !self.has_standard_layout(&path)? {
            return Err(rejected(RecoveryProfileRejection::LayoutCapabilities));
        }
        let catalog_path = path.join(&self.catalog_file_name);
        let profile = self
            .probe
            .probe_recovery_profile(&catalog_path)
            .map_err(profile_probe_error)?;
        if !profile.exists {
            return Err(rejected(RecoveryProfileRejection::CatalogMissing));
        }
        let Some(identity) = profile.identity else {
            return Err(rejected(RecoveryProfileRejection::CatalogIdentityMissing));
        };
        if marker.home_id != identity.home_id {
            return Err(rejected(RecoveryProfileRejection::HomeIdentityMismatch));
        }
        if marker.created_at != identity.created_at {
            return Err(rejected(RecoveryProfileRejection::CreationTimeMismatch));
        }
        if !profile.integrity_ok {
            return Err(rejected(RecoveryProfileRejection::CatalogIntegrity));
        }
        if !profile.foreign_keys_ok {
            return Err(rejected(RecoveryProfileRejection::CatalogForeignKeys));
        }
        if !profile.required_capabilities {
            return Err(rejected(RecoveryProfileRejection::CatalogCapabilities));
        }
        if self.writer_is_active(&catalog_path) {
            return Err(rejected(RecoveryProfileRejection::ActiveWriter));
        }
        if self.operation_recovery_required(&path) {
            return Err(rejected(
                RecoveryProfileRejection::OperationRecoveryRequired,
            ));
        }
        if self
            .classifier
            .classify(&path, FixtureShapeMode::Bound)
            .is_contaminated()
        {
            return Err(rejected(RecoveryProfileRejection::FixtureContamination));
        }
        Ok(VerifiedRecoveryProfile {
            path,
            marker_home_id: marker.home_id,
            marker_created_at: marker.created_at,
        })
    }

    fn read_marker(&self, path: &Path) -> Result<HomeMarker, RecoveryProfileInspectionError> {
        let content = self
            .filesystem
            .read_utf8_file(&path.join(HomeMarker::FILE_NAME))
            .map_err(|error| RecoveryProfileInspectionError::FileSystem(error.to_string()))?;
        content
            .as_deref()
            .and_then(HomeMarker::parse_recovery_profile)
            .ok_or_else(|| rejected(RecoveryProfileRejection::MarkerMissingOrInvalid))
    }

    /// WAL and SHM sidecars are normal SQLite artifacts. Only a busy or
    /// unprobeable WAL-index lock closes recovery; the adapter releases a
    /// successfully acquired lock immediately.
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

    fn has_standard_layout(
        &self,
        home_path: &Path,
    ) -> Result<bool, RecoveryProfileInspectionError> {
        for directory in STANDARD_LAYOUT_DIRS {
            match self
                .filesystem
                .path_is_directory(&home_path.join(directory))
            {
                Ok(true) => {}
                Ok(false) => return Ok(false),
                Err(error) => {
                    return Err(RecoveryProfileInspectionError::FileSystem(
                        error.to_string(),
                    ));
                }
            }
        }
        Ok(true)
    }
}

fn rejected(reason: RecoveryProfileRejection) -> RecoveryProfileInspectionError {
    RecoveryProfileInspectionError::Rejected { reason }
}

fn profile_probe_error(error: CatalogProbeError) -> RecoveryProfileInspectionError {
    match error {
        CatalogProbeError::Unreadable(_) | CatalogProbeError::Invalid(_) => {
            rejected(RecoveryProfileRejection::CatalogUnreadable)
        }
    }
}
