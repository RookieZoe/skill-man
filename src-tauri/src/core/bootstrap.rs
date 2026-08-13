//! Production bootstrap authority (spec §3.3, §5.1): resolves the closed
//! `BootstrapSnapshot` union strictly from App-level state, volume identity,
//! Home marker and a read-only Catalog probe — before any writable open.
//! Fixture classification/recovery, Home Candidate confirmation and legacy
//! transition are owned by later tickets; this module fails closed on every
//! ambiguity and never writes, migrates or seeds.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::core::fixture_recovery::{FixtureClassifier, FixtureShapeMode};
use crate::core::home::{BoundHome, HomeId, HomeMarker};
use crate::core::write_gate::{ClosedReason, ReadOnlyReason, WriteGateState};
use crate::seams::app_state_store::AppStateStore;
use crate::seams::catalog_probe::CatalogProbe;
use crate::seams::filesystem::FileSystem;
use crate::seams::volume_identity::VolumeIdentitySource;

/// Catalog schema that carries Home identity (spec §3.4). Lower schemas are
/// Legacy/unbound states that only the Home Binding flow may migrate.
pub const CATALOG_SCHEMA_WITH_HOME_IDENTITY: u32 = 5;

/// Raw technical detail, never App Copy: rendered in the explicitly labeled
/// diagnostic region with a localized summary elsewhere.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapDiagnostic {
    pub code: String,
    pub message: String,
}

impl BootstrapDiagnostic {
    fn new(code: &str, message: String) -> Self {
        Self {
            code: code.into(),
            message,
        }
    }
}

/// Catalog access resolved by identity verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogAccess {
    ReadWrite,
    ReadOnly { reason: ReadOnlyReason },
}

/// The closed top-level bootstrap route union (spec §4.2). React renders
/// exactly one route per variant; no variant is composed from booleans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BootstrapSnapshot {
    /// App-level state directory/locator/ledger unreadable or contradictory.
    AppStateUnavailable { diagnostic: BootstrapDiagnostic },
    /// No binding, no Legacy Home: the binding wizard entry point. Zero Home
    /// or SQLite artifacts exist.
    Unconfigured,
    /// No binding but the default Legacy path exists: read-only
    /// classification precedes the one-time transition (ticket #45).
    LegacyDetected { path: PathBuf },
    /// Recovery lock held: an active recovery operation exists (ticket #43
    /// resumes it) or fixture contamination blocks regular writes.
    FixtureRecoveryLocked {
        home_id: Option<HomeId>,
        path: Option<PathBuf>,
    },
    /// A confirmed-but-uncommitted candidate (produced by ticket #45's
    /// confirm flow; present in the closed union so routes never guess).
    HomeCandidatePending { path: PathBuf, operation_id: String },
    /// Four-way identity verified (locator + marker + Catalog + volume).
    Bound {
        home_id: HomeId,
        catalog_access: CatalogAccess,
        snapshot_version: u64,
    },
    /// Binding exists but the path or its volume is unreachable.
    HomeUnavailable {
        home_id: HomeId,
        path: PathBuf,
        diagnostic: Option<BootstrapDiagnostic>,
    },
    /// Path reachable but locator/marker/Catalog/volume disagree; the site is
    /// never treated as Bound and nothing is auto-written.
    HomeIdentityMismatch {
        home_id: HomeId,
        path: PathBuf,
        diagnostic: Option<BootstrapDiagnostic>,
    },
}

impl BootstrapSnapshot {
    pub fn is_bound(&self) -> bool {
        matches!(self, BootstrapSnapshot::Bound { .. })
    }

    pub fn write_gate_state(&self, bound_home: Option<&BoundHome>) -> WriteGateState {
        match self {
            BootstrapSnapshot::Bound {
                catalog_access: CatalogAccess::ReadWrite,
                ..
            } => match bound_home {
                // A Bound snapshot without a verified value object must fail
                // closed: the gate capability is never fabricated (spec §4.3).
                Some(bound_home) => WriteGateState::Open(bound_home.clone()),
                None => WriteGateState::Closed {
                    reason: ClosedReason::AppStateUnavailable,
                },
            },
            BootstrapSnapshot::Bound {
                catalog_access: CatalogAccess::ReadOnly { reason },
                ..
            } => WriteGateState::CatalogReadOnly { reason: *reason },
            BootstrapSnapshot::AppStateUnavailable { .. } => WriteGateState::Closed {
                reason: ClosedReason::AppStateUnavailable,
            },
            BootstrapSnapshot::Unconfigured => WriteGateState::Closed {
                reason: ClosedReason::Unconfigured,
            },
            BootstrapSnapshot::LegacyDetected { .. } => WriteGateState::Closed {
                reason: ClosedReason::LegacyDetected,
            },
            BootstrapSnapshot::FixtureRecoveryLocked { .. } => WriteGateState::Closed {
                reason: ClosedReason::FixtureRecoveryLocked,
            },
            BootstrapSnapshot::HomeCandidatePending { .. } => WriteGateState::Closed {
                reason: ClosedReason::HomeCandidatePending,
            },
            BootstrapSnapshot::HomeUnavailable { .. } => WriteGateState::Closed {
                reason: ClosedReason::HomeUnavailable,
            },
            BootstrapSnapshot::HomeIdentityMismatch { .. } => WriteGateState::Closed {
                reason: ClosedReason::HomeIdentityMismatch,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct BootstrapConfig {
    /// `~/Library/Application Support/skill-man-state` — sibling of the
    /// default Home, never inside it.
    pub state_dir: PathBuf,
    /// The default (Legacy) Home path; read-only existence check when no
    /// binding exists.
    pub default_home_path: PathBuf,
    /// Catalog file name inside a Home.
    pub catalog_file_name: String,
}

pub struct BootstrapService {
    app_state: Arc<dyn AppStateStore>,
    volume: Arc<dyn VolumeIdentitySource>,
    probe: Arc<dyn CatalogProbe>,
    filesystem: Arc<dyn FileSystem>,
    classifier: Arc<dyn FixtureClassifier>,
    config: BootstrapConfig,
    /// The most recently verified Bound Home; populated only when `inspect`
    /// resolves to `Bound` and used by composition to open the writable
    /// catalog. Absent in every closed state.
    verified: RwLock<Option<BoundHome>>,
    /// Set when the writable catalog open fails after identity verification;
    /// the session continues read-only and `inspect` reports `OpenFailed`.
    open_failure: RwLock<Option<BootstrapDiagnostic>>,
}

impl BootstrapService {
    pub fn new(
        app_state: Arc<dyn AppStateStore>,
        volume: Arc<dyn VolumeIdentitySource>,
        probe: Arc<dyn CatalogProbe>,
        filesystem: Arc<dyn FileSystem>,
        classifier: Arc<dyn FixtureClassifier>,
        config: BootstrapConfig,
    ) -> Self {
        Self {
            app_state,
            volume,
            probe,
            filesystem,
            classifier,
            config,
            verified: RwLock::new(None),
            open_failure: RwLock::new(None),
        }
    }

    /// Record that the writable open of a verified Bound Home failed; the
    /// session degrades to `CatalogReadOnly { OpenFailed }` and stays there.
    pub fn note_catalog_open_failure(&self, message: String) {
        if let Ok(mut slot) = self.open_failure.write() {
            *slot = Some(BootstrapDiagnostic::new("catalog_open_failed", message));
        }
    }

    /// The last `Bound` Home proven by `inspect`; `None` in every closed
    /// state. Used by composition to construct the `BoundCatalogStore`.
    pub fn verified_bound_home(&self) -> Option<BoundHome> {
        self.verified
            .read()
            .map(|verified| verified.clone())
            .unwrap_or(None)
    }

    /// Resolve the top-level bootstrap state. Read-only: no migration, no
    /// seeding, no locator/marker/DB writes, no Home creation.
    pub fn inspect(&self) -> BootstrapSnapshot {
        let snapshot = self.inspect_inner();
        let verified = match &snapshot {
            BootstrapSnapshot::Bound { home_id, .. } => self.build_bound_home(home_id),
            _ => None,
        };
        if let Ok(mut slot) = self.verified.write() {
            *slot = verified.clone();
        }
        snapshot
    }

    fn inspect_inner(&self) -> BootstrapSnapshot {
        // §5.1 step 2: App-level state must parse before anything else.
        let files = match self.app_state.load() {
            Ok(files) => files,
            Err(error) => {
                return BootstrapSnapshot::AppStateUnavailable {
                    diagnostic: BootstrapDiagnostic::new(
                        "app_state_unavailable",
                        error.to_string(),
                    ),
                };
            }
        };

        // §5.1 step 6: an active recovery operation keeps the recovery lock;
        // only the Fixture Recovery ticket resumes or completes it.
        if let Some(active) = &files.recovery_ledger.active {
            return BootstrapSnapshot::FixtureRecoveryLocked {
                home_id: active.home_id.clone(),
                path: active.live_path.clone(),
            };
        }

        // §5.1 step 4: no binding.
        let Some(current) = files.binding.current else {
            let legacy_exists = self
                .filesystem
                .path_is_directory(&self.config.default_home_path)
                .unwrap_or(false);
            if legacy_exists {
                // §5.1 step 7: fixture classification precedes the legacy
                // transition; any footprint keeps the recovery lock.
                let classification = self
                    .classifier
                    .classify(&self.config.default_home_path, FixtureShapeMode::Legacy);
                if classification.is_contaminated() {
                    return BootstrapSnapshot::FixtureRecoveryLocked {
                        home_id: None,
                        path: Some(self.config.default_home_path.clone()),
                    };
                }
                return BootstrapSnapshot::LegacyDetected {
                    path: self.config.default_home_path.clone(),
                };
            }
            return BootstrapSnapshot::Unconfigured;
        };

        let home_id = current.home_id.clone();
        let path = current.path.clone();

        // §3.3: volume reachability first.
        let volume = match self.volume.volume_identity(&path) {
            Ok(Some(volume)) => volume,
            Ok(None) => {
                let message = format!("no volume identity for {}", path.display());
                return BootstrapSnapshot::HomeUnavailable {
                    home_id,
                    path,
                    diagnostic: Some(BootstrapDiagnostic::new("volume_unreachable", message)),
                };
            }
            Err(error) => {
                return BootstrapSnapshot::HomeUnavailable {
                    home_id,
                    path,
                    diagnostic: Some(BootstrapDiagnostic::new(
                        "volume_unreachable",
                        error.to_string(),
                    )),
                };
            }
        };
        if volume.fsid != current.volume_fsid || volume.uuid != current.volume_uuid {
            return BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                path,
                diagnostic: Some(BootstrapDiagnostic::new(
                    "volume_identity_mismatch",
                    format!(
                        "bound volume ({} / {}) differs from current ({} / {})",
                        current.volume_fsid, current.volume_uuid, volume.fsid, volume.uuid
                    ),
                )),
            };
        }

        // §3.3: marker must agree with the locator and the current volume.
        let marker_path = path.join(HomeMarker::FILE_NAME);
        let marker = match self.read_marker(&marker_path) {
            Some(marker) => marker,
            None => {
                return BootstrapSnapshot::HomeIdentityMismatch {
                    home_id,
                    path,
                    diagnostic: Some(BootstrapDiagnostic::new(
                        "home_marker_invalid",
                        format!("{} is missing or invalid", marker_path.display()),
                    )),
                };
            }
        };
        if marker.home_id != current.home_id || !marker.matches_volume(&volume) {
            return BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                path,
                diagnostic: Some(BootstrapDiagnostic::new(
                    "home_marker_identity_mismatch",
                    "the Home marker does not match the binding or the current volume".into(),
                )),
            };
        }

        // §3.3/§3.4: read-only Catalog probe; identity must agree before any
        // writable open.
        let catalog_path = path.join(&self.config.catalog_file_name);
        let report = match self.probe.probe(&catalog_path) {
            Ok(report) => report,
            Err(error) => {
                return BootstrapSnapshot::HomeIdentityMismatch {
                    home_id,
                    path,
                    diagnostic: Some(BootstrapDiagnostic::new(
                        "catalog_unreadable",
                        error.to_string(),
                    )),
                };
            }
        };
        if !report.exists {
            return BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                path,
                diagnostic: Some(BootstrapDiagnostic::new(
                    "catalog_missing",
                    format!("{} does not exist", catalog_path.display()),
                )),
            };
        }
        let Some(schema_version) = report.schema_version else {
            return BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                path,
                diagnostic: Some(BootstrapDiagnostic::new(
                    "catalog_schema_unknown",
                    "the Catalog schema could not be identified".into(),
                )),
            };
        };
        if schema_version < CATALOG_SCHEMA_WITH_HOME_IDENTITY {
            return BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                path,
                diagnostic: Some(BootstrapDiagnostic::new(
                    "catalog_needs_binding_transition",
                    format!("Catalog schema v{schema_version} predates Home identity"),
                )),
            };
        }
        let Some(catalog_identity) = report.home_identity else {
            return BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                path,
                diagnostic: Some(BootstrapDiagnostic::new(
                    "catalog_identity_missing",
                    "the Catalog has no Home identity recorded".into(),
                )),
            };
        };
        if catalog_identity.home_id != current.home_id
            || catalog_identity.volume_fsid != current.volume_fsid
            || catalog_identity.volume_uuid != current.volume_uuid
        {
            return BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                path,
                diagnostic: Some(BootstrapDiagnostic::new(
                    "catalog_identity_mismatch",
                    "the Catalog identity does not match the binding".into(),
                )),
            };
        }

        // §5.1 step 7: a verified Bound Home with a fixture footprint keeps
        // the recovery lock (Bound Restore mode) instead of opening.
        let classification = self.classifier.classify(&path, FixtureShapeMode::Bound);
        if classification.is_contaminated() {
            return BootstrapSnapshot::FixtureRecoveryLocked {
                home_id: Some(home_id),
                path: Some(path),
            };
        }

        let mut catalog_access =
            if schema_version > crate::seams::catalog_probe::CURRENT_CATALOG_SCHEMA_VERSION {
                CatalogAccess::ReadOnly {
                    reason: ReadOnlyReason::UnsupportedSchema,
                }
            } else if !report.integrity_ok || !report.foreign_keys_ok {
                CatalogAccess::ReadOnly {
                    reason: ReadOnlyReason::IntegrityFailed,
                }
            } else {
                CatalogAccess::ReadWrite
            };
        if self
            .open_failure
            .read()
            .ok()
            .and_then(|failure| failure.clone())
            .is_some()
        {
            catalog_access = CatalogAccess::ReadOnly {
                reason: ReadOnlyReason::OpenFailed,
            };
        }
        BootstrapSnapshot::Bound {
            home_id,
            catalog_access,
            snapshot_version: report.snapshot_version.unwrap_or(0),
        }
    }

    fn read_marker(&self, path: &std::path::Path) -> Option<HomeMarker> {
        let content = self.filesystem.read_utf8_file(path).ok()??;
        HomeMarker::parse(&content)
    }

    fn build_bound_home(&self, home_id: &HomeId) -> Option<BoundHome> {
        let files = self.app_state.load().ok()?;
        let current = files.binding.current?;
        if current.home_id != *home_id {
            return None;
        }
        Some(BoundHome {
            home_id: current.home_id,
            path: current.path,
            volume_fsid: current.volume_fsid,
            volume_uuid: current.volume_uuid,
            bound_at: current.bound_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use crate::adapters::macos_fs::MacOsFileSystem;
    use crate::core::fixture_recovery::{
        FixtureClassification, FixtureClassifier, FixtureShapeMode,
    };
    use crate::core::home::VolumeIdentity;
    use crate::seams::app_state_store::{
        AppStateFiles, AppStateStoreError, HomeBindingFile, HomeBindingRecord, RecoveryLedgerFile,
        RecoveryOperationRecord,
    };
    use crate::seams::catalog_probe::{
        CatalogHomeIdentity, CatalogProbe, CatalogProbeError, CatalogProbeReport,
    };
    use crate::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

    use super::*;

    const HOME_ID: &str = "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";

    struct MemoryAppStateStore {
        state: Mutex<AppStateFiles>,
        fail: Mutex<bool>,
    }

    impl MemoryAppStateStore {
        fn new(files: AppStateFiles) -> Self {
            Self {
                state: Mutex::new(files),
                fail: Mutex::new(false),
            }
        }
    }

    impl AppStateStore for MemoryAppStateStore {
        fn load(&self) -> Result<AppStateFiles, AppStateStoreError> {
            if *self.fail.lock().unwrap() {
                return Err(AppStateStoreError::LocatorInvalid(
                    "injected failure".into(),
                ));
            }
            Ok(self.state.lock().unwrap().clone())
        }

        fn write_locator(&self, binding: &HomeBindingFile) -> Result<(), AppStateStoreError> {
            let mut state = self.state.lock().unwrap();
            state.binding = binding.clone();
            Ok(())
        }

        fn write_recovery_ledger(
            &self,
            ledger: &RecoveryLedgerFile,
        ) -> Result<(), AppStateStoreError> {
            let mut state = self.state.lock().unwrap();
            state.recovery_ledger = ledger.clone();
            Ok(())
        }
    }

    struct FixedVolumeIdentitySource {
        volume: Option<VolumeIdentity>,
        fail: bool,
    }

    impl VolumeIdentitySource for FixedVolumeIdentitySource {
        fn volume_identity(
            &self,
            _path: &Path,
        ) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
            if self.fail {
                return Err(VolumeIdentityError::Unavailable {
                    path: "injected".into(),
                    detail: "injected failure".into(),
                });
            }
            Ok(self.volume.clone())
        }
    }

    struct MemoryCatalogProbe {
        reports: Mutex<HashMap<PathBuf, CatalogProbeReport>>,
    }

    impl MemoryCatalogProbe {
        fn new(report: CatalogProbeReport) -> Self {
            let mut reports = HashMap::new();
            reports.insert(PathBuf::from("catalog"), report);
            Self {
                reports: Mutex::new(reports),
            }
        }
    }

    impl CatalogProbe for MemoryCatalogProbe {
        fn probe(&self, _path: &Path) -> Result<CatalogProbeReport, CatalogProbeError> {
            self.reports
                .lock()
                .unwrap()
                .get(&PathBuf::from("catalog"))
                .cloned()
                .map(Ok)
                .unwrap_or(Ok(CatalogProbeReport::absent()))
        }
    }

    struct FixedClassifier(FixtureClassification);

    impl FixtureClassifier for FixedClassifier {
        fn classify(
            &self,
            _home_root: &std::path::Path,
            _mode: FixtureShapeMode,
        ) -> FixtureClassification {
            self.0.clone()
        }
    }

    fn bound_files(home_path: &Path) -> AppStateFiles {
        AppStateFiles {
            binding: HomeBindingFile {
                schema_version: 1,
                current: Some(HomeBindingRecord {
                    home_id: HomeId(HOME_ID.into()),
                    path: home_path.to_path_buf(),
                    volume_fsid: "fsid-1".into(),
                    volume_uuid: "uuid-1".into(),
                    bound_at: "2026-08-01T00:00:00Z".into(),
                }),
                abandoned: vec![],
            },
            recovery_ledger: RecoveryLedgerFile::empty(),
        }
    }

    fn probe_report(schema: u32, identity: Option<CatalogHomeIdentity>) -> CatalogProbeReport {
        CatalogProbeReport {
            exists: true,
            schema_version: Some(schema),
            integrity_ok: true,
            foreign_keys_ok: true,
            home_identity: identity,
            snapshot_version: Some(3),
        }
    }

    fn matching_identity() -> CatalogHomeIdentity {
        CatalogHomeIdentity {
            home_id: HomeId(HOME_ID.into()),
            volume_fsid: "fsid-1".into(),
            volume_uuid: "uuid-1".into(),
            home_bound_at: "2026-08-01T00:00:00Z".into(),
        }
    }

    fn service(
        app_state: MemoryAppStateStore,
        volume: FixedVolumeIdentitySource,
        probe: MemoryCatalogProbe,
        marker: Option<&str>,
        marker_dir: &Path,
    ) -> BootstrapService {
        let filesystem = MacOsFileSystem::new(marker_dir.to_path_buf());
        if let Some(marker) = marker {
            filesystem.create_directory(marker_dir).expect("marker dir");
            filesystem
                .write_utf8_file(&marker_dir.join(HomeMarker::FILE_NAME), marker)
                .expect("marker file");
        }
        BootstrapService::new(
            Arc::new(app_state),
            Arc::new(volume),
            Arc::new(probe),
            Arc::new(filesystem),
            Arc::new(FixedClassifier(FixtureClassification::Clean)),
            BootstrapConfig {
                state_dir: marker_dir.join("state"),
                default_home_path: marker_dir.join("default-home"),
                catalog_file_name: "skill-man.sqlite3".into(),
            },
        )
    }

    fn valid_marker() -> String {
        format!(
            r#"{{
                "schema_version": 1,
                "home_id": "{HOME_ID}",
                "volume_fsid": "fsid-1",
                "volume_uuid": "uuid-1",
                "created_at": "2026-08-01T00:00:00Z"
            }}"#
        )
    }

    #[test]
    fn fresh_state_without_binding_is_unconfigured() {
        let files = AppStateFiles {
            binding: HomeBindingFile::empty(),
            recovery_ledger: RecoveryLedgerFile::empty(),
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let service = service(
            MemoryAppStateStore::new(files),
            FixedVolumeIdentitySource {
                volume: None,
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            None,
            dir.path(),
        );
        assert_eq!(service.inspect(), BootstrapSnapshot::Unconfigured);
        assert!(service.verified_bound_home().is_none());
    }

    #[test]
    fn unreadable_app_state_is_app_state_unavailable() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let mut store = MemoryAppStateStore::new(bound_files(&home));
        *store.fail.get_mut().unwrap() = true;
        let service = service(
            store,
            FixedVolumeIdentitySource {
                volume: None,
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            None,
            dir.path(),
        );
        let snapshot = service.inspect();
        match snapshot {
            BootstrapSnapshot::AppStateUnavailable { diagnostic } => {
                assert_eq!(diagnostic.code, "app_state_unavailable");
            }
            other => panic!("expected AppStateUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn active_recovery_operation_keeps_the_recovery_lock() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let mut files = bound_files(&home);
        files.recovery_ledger.active = Some(RecoveryOperationRecord {
            operation_id: "op-1".into(),
            kind: "fixture_recovery".into(),
            home_id: Some(HomeId(HOME_ID.into())),
            live_path: Some(home.clone()),
            snapshot_path: None,
            prepared_path: None,
            manifest_hash: None,
            external_probe: None,
            cursor: None,
            commit_point: None,
            created_at: "2026-08-01T00:00:00Z".into(),
        });
        let dir = tempfile::tempdir().expect("temp dir");
        let service = service(
            MemoryAppStateStore::new(files),
            FixedVolumeIdentitySource {
                volume: None,
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            None,
            dir.path(),
        );
        match service.inspect() {
            BootstrapSnapshot::FixtureRecoveryLocked { home_id, .. } => {
                assert_eq!(home_id.as_ref().map(|id| id.0.as_str()), Some(HOME_ID));
            }
            other => panic!("expected FixtureRecoveryLocked, got {other:?}"),
        }
    }

    #[test]
    fn no_binding_with_legacy_path_is_legacy_detected() {
        let files = AppStateFiles {
            binding: HomeBindingFile::empty(),
            recovery_ledger: RecoveryLedgerFile::empty(),
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let filesystem = MacOsFileSystem::new(dir.path().to_path_buf());
        filesystem
            .create_directory(&dir.path().join("default-home"))
            .expect("legacy dir");
        let service = service(
            MemoryAppStateStore::new(files),
            FixedVolumeIdentitySource {
                volume: None,
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            None,
            dir.path(),
        );
        match service.inspect() {
            BootstrapSnapshot::LegacyDetected { path } => {
                assert_eq!(path, dir.path().join("default-home"));
            }
            other => panic!("expected LegacyDetected, got {other:?}"),
        }
    }

    #[test]
    fn contaminated_legacy_path_is_fixture_recovery_locked() {
        let files = AppStateFiles {
            binding: HomeBindingFile::empty(),
            recovery_ledger: RecoveryLedgerFile::empty(),
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let filesystem = MacOsFileSystem::new(dir.path().to_path_buf());
        filesystem
            .create_directory(&dir.path().join("default-home"))
            .expect("legacy dir");
        let service = BootstrapService::new(
            Arc::new(MemoryAppStateStore::new(files)),
            Arc::new(FixedVolumeIdentitySource {
                volume: None,
                fail: false,
            }),
            Arc::new(MemoryCatalogProbe::new(CatalogProbeReport::absent())),
            Arc::new(filesystem),
            Arc::new(FixedClassifier(FixtureClassification::Mixed {
                reasons: vec!["fixture_entities_missing".into()],
            })),
            BootstrapConfig {
                state_dir: dir.path().join("state"),
                default_home_path: dir.path().join("default-home"),
                catalog_file_name: "skill-man.sqlite3".into(),
            },
        );
        match service.inspect() {
            BootstrapSnapshot::FixtureRecoveryLocked { home_id, path } => {
                assert!(home_id.is_none(), "Legacy lock carries no Home identity");
                assert_eq!(path, Some(dir.path().join("default-home")));
            }
            other => panic!("expected FixtureRecoveryLocked, got {other:?}"),
        }
    }

    #[test]
    fn contaminated_bound_home_is_fixture_recovery_locked() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let filesystem = MacOsFileSystem::new(dir.path().to_path_buf());
        filesystem.create_directory(&home).expect("home dir");
        filesystem
            .write_utf8_file(&home.join(HomeMarker::FILE_NAME), &valid_marker())
            .expect("marker file");
        let service = BootstrapService::new(
            Arc::new(MemoryAppStateStore::new(bound_files(&home))),
            Arc::new(FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            }),
            Arc::new(MemoryCatalogProbe::new(probe_report(
                5,
                Some(matching_identity()),
            ))),
            Arc::new(filesystem),
            Arc::new(FixedClassifier(FixtureClassification::Pure)),
            BootstrapConfig {
                state_dir: dir.path().join("state"),
                default_home_path: dir.path().join("default-home"),
                catalog_file_name: "skill-man.sqlite3".into(),
            },
        );
        match service.inspect() {
            BootstrapSnapshot::FixtureRecoveryLocked { home_id, path } => {
                assert_eq!(
                    home_id.as_ref().map(|id| id.0.as_str()),
                    Some(HOME_ID),
                    "Bound lock carries the bound identity"
                );
                assert_eq!(path, Some(home));
            }
            other => panic!("expected FixtureRecoveryLocked, got {other:?}"),
        }
        assert!(
            service.verified_bound_home().is_none(),
            "a locked Home never yields the writable value object"
        );
    }

    #[test]
    fn unreachable_volume_is_home_unavailable() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: None,
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            Some(&valid_marker()),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::HomeUnavailable {
                home_id,
                diagnostic,
                ..
            } => {
                assert_eq!(home_id.0, HOME_ID);
                assert_eq!(
                    diagnostic.as_ref().map(|d| d.code.as_str()),
                    Some("volume_unreachable")
                );
            }
            other => panic!("expected HomeUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn volume_identity_failure_is_home_unavailable() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: true,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            Some(&valid_marker()),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::HomeUnavailable { .. } => {}
            other => panic!("expected HomeUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn changed_volume_is_home_identity_mismatch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-other".into(),
                    uuid: "uuid-other".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            Some(&valid_marker()),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::HomeIdentityMismatch {
                home_id,
                diagnostic,
                ..
            } => {
                assert_eq!(home_id.0, HOME_ID);
                assert_eq!(
                    diagnostic.as_ref().map(|d| d.code.as_str()),
                    Some("volume_identity_mismatch")
                );
            }
            other => panic!("expected HomeIdentityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_marker_is_home_identity_mismatch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            None,
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
                assert_eq!(
                    diagnostic.as_ref().map(|d| d.code.as_str()),
                    Some("home_marker_invalid")
                );
            }
            other => panic!("expected HomeIdentityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn mismatched_marker_identity_is_home_identity_mismatch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let other_marker = valid_marker().replace(HOME_ID, "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            Some(&other_marker),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
                assert_eq!(
                    diagnostic.as_ref().map(|d| d.code.as_str()),
                    Some("home_marker_identity_mismatch")
                );
            }
            other => panic!("expected HomeIdentityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_catalog_is_home_identity_mismatch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(CatalogProbeReport::absent()),
            Some(&valid_marker()),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
                assert_eq!(
                    diagnostic.as_ref().map(|d| d.code.as_str()),
                    Some("catalog_missing")
                );
            }
            other => panic!("expected HomeIdentityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn legacy_catalog_schema_is_home_identity_mismatch_not_bound() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(probe_report(4, None)),
            Some(&valid_marker()),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
                assert_eq!(
                    diagnostic.as_ref().map(|d| d.code.as_str()),
                    Some("catalog_needs_binding_transition")
                );
            }
            other => panic!("expected HomeIdentityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn matching_identity_is_bound_read_write() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(probe_report(5, Some(matching_identity()))),
            Some(&valid_marker()),
            &home,
        );
        let snapshot = service.inspect();
        match &snapshot {
            BootstrapSnapshot::Bound {
                catalog_access: CatalogAccess::ReadWrite,
                snapshot_version,
                ..
            } => assert_eq!(*snapshot_version, 3),
            other => panic!("expected Bound ReadWrite, got {other:?}"),
        }
        let bound = service.verified_bound_home().expect("verified Home");
        assert_eq!(bound.home_id.0, HOME_ID);
        assert_eq!(
            snapshot.write_gate_state(Some(&bound)),
            WriteGateState::Open(bound.clone())
        );
    }

    #[test]
    fn bound_without_a_verified_home_fails_closed_never_fabricates() {
        let snapshot = BootstrapSnapshot::Bound {
            home_id: HomeId(HOME_ID.into()),
            catalog_access: CatalogAccess::ReadWrite,
            snapshot_version: 3,
        };
        assert_eq!(
            snapshot.write_gate_state(None),
            WriteGateState::Closed {
                reason: ClosedReason::AppStateUnavailable
            },
            "the gate capability must never be fabricated from a test value"
        );
    }

    #[test]
    fn mismatched_catalog_identity_is_home_identity_mismatch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let mut identity = matching_identity();
        identity.volume_uuid = "uuid-other".into();
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(probe_report(5, Some(identity))),
            Some(&valid_marker()),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::HomeIdentityMismatch { diagnostic, .. } => {
                assert_eq!(
                    diagnostic.as_ref().map(|d| d.code.as_str()),
                    Some("catalog_identity_mismatch")
                );
            }
            other => panic!("expected HomeIdentityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_future_schema_is_bound_read_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(probe_report(9, Some(matching_identity()))),
            Some(&valid_marker()),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::Bound {
                catalog_access: CatalogAccess::ReadOnly { reason },
                ..
            } => assert_eq!(reason, ReadOnlyReason::UnsupportedSchema),
            other => panic!("expected Bound ReadOnly, got {other:?}"),
        }
    }

    #[test]
    fn integrity_failure_is_bound_read_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let mut report = probe_report(5, Some(matching_identity()));
        report.integrity_ok = false;
        let service = service(
            MemoryAppStateStore::new(bound_files(&home)),
            FixedVolumeIdentitySource {
                volume: Some(VolumeIdentity {
                    fsid: "fsid-1".into(),
                    uuid: "uuid-1".into(),
                }),
                fail: false,
            },
            MemoryCatalogProbe::new(report),
            Some(&valid_marker()),
            &home,
        );
        match service.inspect() {
            BootstrapSnapshot::Bound {
                catalog_access: CatalogAccess::ReadOnly { reason },
                ..
            } => assert_eq!(reason, ReadOnlyReason::IntegrityFailed),
            other => panic!("expected Bound ReadOnly, got {other:?}"),
        }
    }
}
