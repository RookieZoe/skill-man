//! App-level state authority seam (spec §3.2, §4.2): the bootstrap locator
//! and the durable recovery ledger live outside any Home so binding state is
//! never hostage to a Home path. `LocaleStore` is a separate seam owned by the
//! locale ticket.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::home::HomeId;

pub const HOME_BINDING_FILE_NAME: &str = "home-binding.json";
pub const RECOVERY_LEDGER_FILE_NAME: &str = "recovery-ledger.json";
pub const HOME_BINDING_SCHEMA_VERSION: u32 = 1;
pub const RECOVERY_LEDGER_SCHEMA_VERSION: u32 = 1;

/// The active binding record inside `home-binding.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HomeBindingRecord {
    pub home_id: HomeId,
    pub path: PathBuf,
    pub volume_fsid: String,
    pub volume_uuid: String,
    pub bound_at: String,
}

/// A permanently abandoned binding; kept for history, never re-bound.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AbandonedHomeRecord {
    pub home_id: HomeId,
    pub path: PathBuf,
    pub volume_fsid: String,
    pub volume_uuid: String,
    pub abandoned_at: String,
}

/// Parsed `home-binding.json`: the single truth for binding state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HomeBindingFile {
    pub schema_version: u32,
    pub current: Option<HomeBindingRecord>,
    pub abandoned: Vec<AbandonedHomeRecord>,
}

impl HomeBindingFile {
    /// Fresh state: no locator file means no binding and no history.
    pub fn empty() -> Self {
        Self {
            schema_version: HOME_BINDING_SCHEMA_VERSION,
            current: None,
            abandoned: Vec::new(),
        }
    }

    /// Strict parse per §3.2: unknown schema, malformed identity, relative
    /// paths, empty volume identity or a `home_id` present on both sides of
    /// the ledger make the whole file invalid (`AppStateUnavailable`) — never
    /// a guess or a rebuild.
    pub fn parse(json: &str) -> Option<Self> {
        let file: HomeBindingFile = serde_json::from_str(json).ok()?;
        if file.schema_version != HOME_BINDING_SCHEMA_VERSION {
            return None;
        }
        let mut seen = std::collections::HashSet::new();
        if let Some(current) = &file.current {
            if !valid_record(
                &current.home_id,
                &current.path,
                &current.volume_fsid,
                &current.volume_uuid,
                &current.bound_at,
            ) {
                return None;
            }
            seen.insert(current.home_id.clone());
        }
        for abandoned in &file.abandoned {
            if !valid_record(
                &abandoned.home_id,
                &abandoned.path,
                &abandoned.volume_fsid,
                &abandoned.volume_uuid,
                &abandoned.abandoned_at,
            ) {
                return None;
            }
            if !seen.insert(abandoned.home_id.clone()) {
                return None;
            }
        }
        Some(file)
    }
}

fn valid_record(
    home_id: &HomeId,
    path: &Path,
    volume_fsid: &str,
    volume_uuid: &str,
    recorded_at: &str,
) -> bool {
    HomeId::parse(&home_id.0).is_some()
        && path.is_absolute()
        && !volume_fsid.is_empty()
        && !volume_uuid.is_empty()
        && !recorded_at.is_empty()
}

/// A single durable recovery operation (§3.2). Fixture Recovery (ticket #43)
/// owns the full state machine; bootstrap only needs to know whether an
/// operation is active so it can keep the recovery lock.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecoveryOperationRecord {
    pub operation_id: String,
    pub kind: String,
    pub home_id: Option<HomeId>,
    pub live_path: Option<PathBuf>,
    pub snapshot_path: Option<PathBuf>,
    pub prepared_path: Option<PathBuf>,
    pub manifest_hash: Option<String>,
    /// Targeted lstat probe of the external app-state directory (excluding
    /// the ledger itself), taken before the operation mutates the Home and
    /// re-checked at promote/verify/commit (§5.2 "external tree unchanged").
    #[serde(default)]
    pub external_probe: Option<String>,
    pub cursor: Option<String>,
    pub commit_point: Option<String>,
    pub created_at: String,
}

impl RecoveryOperationRecord {
    fn is_valid(&self) -> bool {
        if self.operation_id.is_empty() || self.kind.is_empty() || self.created_at.is_empty() {
            return false;
        }
        if let Some(home_id) = &self.home_id {
            if HomeId::parse(&home_id.0).is_none() {
                return false;
            }
        }
        true
    }
}

/// Parsed `recovery-ledger.json`: at most one active operation plus an
/// append-only completed history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecoveryLedgerFile {
    pub schema_version: u32,
    pub active: Option<RecoveryOperationRecord>,
    pub completed: Vec<RecoveryOperationRecord>,
}

impl RecoveryLedgerFile {
    pub fn empty() -> Self {
        Self {
            schema_version: RECOVERY_LEDGER_SCHEMA_VERSION,
            active: None,
            completed: Vec::new(),
        }
    }

    pub fn parse(json: &str) -> Option<Self> {
        let file: RecoveryLedgerFile = serde_json::from_str(json).ok()?;
        if file.schema_version != RECOVERY_LEDGER_SCHEMA_VERSION {
            return None;
        }
        if let Some(active) = &file.active {
            if !active.is_valid() {
                return None;
            }
        }
        if file.completed.iter().any(|record| !record.is_valid()) {
            return None;
        }
        Some(file)
    }
}

/// The two app-level files read together at bootstrap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppStateFiles {
    pub binding: HomeBindingFile,
    pub recovery_ledger: RecoveryLedgerFile,
}

#[derive(Debug, Error)]
pub enum AppStateStoreError {
    #[error("the app state directory is unreadable: {0}")]
    StateDirUnreadable(String),
    #[error("the bootstrap locator is unreadable or invalid: {0}")]
    LocatorInvalid(String),
    #[error("the recovery ledger is unreadable or invalid: {0}")]
    LedgerInvalid(String),
    #[error("could not write the bootstrap locator: {0}")]
    WriteFailed(String),
    #[error(
        "the bootstrap locator changed since it was read; expected current {expected:?}, found {found:?}"
    )]
    LocatorCasConflict {
        expected: Option<String>,
        found: Option<String>,
    },
}

/// Seam: atomic read/CAS write of the locator and the recovery ledger. The
/// system adapter implements the tmp → fsync → rename → parent fsync protocol
/// (§3.2); a fault-injecting temp-dir adapter proves crash behavior.
pub trait AppStateStore: Send + Sync {
    /// Reads and strictly validates both files. A missing file is the empty
    /// state; an unreadable directory or an invalid file is an error — the
    /// bootstrap must never guess or rebuild state.
    fn load(&self) -> Result<AppStateFiles, AppStateStoreError>;

    /// Atomically persists the locator. Used by the Home Binding flow to
    /// commit a binding and by Abandon to move `current` into history.
    fn write_locator(&self, binding: &HomeBindingFile) -> Result<(), AppStateStoreError>;

    /// Atomic compare-and-swap of the locator's `current` binding
    /// (ADR-0012 §6): the Abandon commit point. Succeeds only when the
    /// stored `current` still matches `expected_current` (`None` = no
    /// current binding); a concurrent binding commit or a second Abandon
    /// surfaces `LocatorCasConflict` and the caller's `next` is never
    /// written. The system adapter's read-verify-write is serialized by the
    /// single-writer app; the fault-injecting test adapter proves both
    /// failure directions.
    fn cas_locator(
        &self,
        expected_current: Option<&HomeId>,
        next: &HomeBindingFile,
    ) -> Result<(), AppStateStoreError> {
        let files = self.load()?;
        let matches = match (expected_current, &files.binding.current) {
            (None, None) => true,
            (Some(expected), Some(current)) => &current.home_id == expected,
            _ => false,
        };
        if !matches {
            return Err(AppStateStoreError::LocatorCasConflict {
                expected: expected_current.map(|id| id.0.clone()),
                found: files.binding.current.map(|current| current.home_id.0),
            });
        }
        self.write_locator(next)
    }

    /// CAS for rebuilding a lost locator during Existing Home Recovery. This
    /// is intentionally stricter than `cas_locator(None, ...)`: the state
    /// must still be the complete `Unconfigured` state — no current binding,
    /// no abandoned history and no active recovery ledger. Implementations
    /// must test the whole predicate and persist `next` under one serialized
    /// critical section; a concurrent history or operation must never be
    /// overwritten merely because `current` is absent.
    fn cas_unconfigured_locator(&self, next: &HomeBindingFile) -> Result<(), AppStateStoreError> {
        let _ = next;
        Err(AppStateStoreError::WriteFailed(
            "AppStateStore must provide an atomic complete-Unconfigured locator CAS".into(),
        ))
    }

    /// Atomically persists the recovery ledger (same protocol as the
    /// locator). Every durable cursor of a recovery operation goes through
    /// here — the ledger is the single commit point for the state machine.
    fn write_recovery_ledger(&self, ledger: &RecoveryLedgerFile) -> Result<(), AppStateStoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_json() -> String {
        r#"{
            "schema_version": 1,
            "current": {
                "home_id": "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
                "path": "/Users/zoe/Library/Application Support/skill-man",
                "volume_fsid": "fsid-1",
                "volume_uuid": "uuid-1",
                "bound_at": "2026-08-01T00:00:00Z"
            },
            "abandoned": []
        }"#
        .into()
    }

    #[test]
    fn locator_parse_accepts_valid_binding() {
        let file = HomeBindingFile::parse(&valid_json()).expect("valid locator");
        assert_eq!(
            file.current.as_ref().expect("current binding").home_id,
            HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into())
        );
    }

    #[test]
    fn locator_parse_rejects_unknown_schema() {
        let json = valid_json().replace("\"schema_version\": 1", "\"schema_version\": 2");
        assert!(HomeBindingFile::parse(&json).is_none());
    }

    #[test]
    fn locator_parse_rejects_relative_path_and_missing_volume() {
        let json = valid_json().replace(
            "\"/Users/zoe/Library/Application Support/skill-man\"",
            "\"relative/home\"",
        );
        assert!(HomeBindingFile::parse(&json).is_none());

        let json = valid_json().replace("\"volume_uuid\": \"uuid-1\",\n", "");
        assert!(HomeBindingFile::parse(&json).is_none());
    }

    #[test]
    fn locator_parse_rejects_same_home_id_in_current_and_abandoned() {
        let json = r#"{
            "schema_version": 1,
            "current": {
                "home_id": "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
                "path": "/a",
                "volume_fsid": "f",
                "volume_uuid": "u",
                "bound_at": "2026-08-01T00:00:00Z"
            },
            "abandoned": [{
                "home_id": "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
                "path": "/b",
                "volume_fsid": "f",
                "volume_uuid": "u",
                "abandoned_at": "2026-08-02T00:00:00Z"
            }]
        }"#;
        assert!(HomeBindingFile::parse(json).is_none());
    }

    #[test]
    fn cas_locator_only_succeeds_on_the_expected_current_binding() {
        use crate::core::home::HomeId;

        let dir = tempfile::tempdir().expect("temp dir");
        let store = crate::adapters::app_state_store::AppStateStoreFileSystem::new(
            dir.path().join("state"),
        );
        let current_id = HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into());
        let binding = HomeBindingFile {
            schema_version: 1,
            current: Some(HomeBindingRecord {
                home_id: current_id.clone(),
                path: std::path::PathBuf::from("/tmp/skill-man-home"),
                volume_fsid: "fsid-1".into(),
                volume_uuid: "uuid-1".into(),
                bound_at: "2026-08-01T00:00:00Z".into(),
            }),
            abandoned: vec![],
        };
        store.write_locator(&binding).expect("seed locator");

        // Abandon: move the expected current into history.
        let abandoned = HomeBindingFile {
            schema_version: 1,
            current: None,
            abandoned: vec![AbandonedHomeRecord {
                home_id: current_id.clone(),
                path: std::path::PathBuf::from("/tmp/skill-man-home"),
                volume_fsid: "fsid-1".into(),
                volume_uuid: "uuid-1".into(),
                abandoned_at: "2026-08-13T10:00:00Z".into(),
            }],
        };
        store
            .cas_locator(Some(&current_id), &abandoned)
            .expect("CAS with the expected current succeeds");

        // A second Abandon (or a binding commit) races: the current is gone,
        // so the same CAS is refused and nothing is overwritten.
        match store.cas_locator(Some(&current_id), &abandoned) {
            Err(AppStateStoreError::LocatorCasConflict { found, .. }) => {
                assert_eq!(found, None, "no current binding remains");
            }
            other => panic!("expected LocatorCasConflict, got {other:?}"),
        }
        let files = store.load().expect("reload");
        assert_eq!(files.binding, abandoned, "the CAS result is untouched");

        // A None expectation is refused while a binding exists.
        store
            .write_locator(&binding)
            .expect("re-seed a current binding");
        match store.cas_locator(None, &abandoned) {
            Err(AppStateStoreError::LocatorCasConflict { expected, found }) => {
                assert_eq!(expected, None);
                assert_eq!(found.as_deref(), Some(current_id.as_str()));
            }
            other => panic!("expected LocatorCasConflict, got {other:?}"),
        }
    }

    #[test]
    fn unconfigured_locator_cas_refuses_abandoned_history_without_overwriting_it() {
        use crate::core::home::HomeId;

        let dir = tempfile::tempdir().expect("temp dir");
        let store = crate::adapters::app_state_store::AppStateStoreFileSystem::new(
            dir.path().join("state"),
        );
        let abandoned = HomeBindingFile {
            schema_version: 1,
            current: None,
            abandoned: vec![AbandonedHomeRecord {
                home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
                path: std::path::PathBuf::from("/tmp/skill-man-home"),
                volume_fsid: "fsid-1".into(),
                volume_uuid: "uuid-1".into(),
                abandoned_at: "2026-08-13T10:00:00Z".into(),
            }],
        };
        store.write_locator(&abandoned).expect("seed history");

        match store.cas_unconfigured_locator(&HomeBindingFile::empty()) {
            Err(AppStateStoreError::LocatorCasConflict { expected, found }) => {
                assert_eq!(expected, None);
                assert_eq!(found, None);
            }
            other => panic!("expected complete-Unconfigured CAS conflict, got {other:?}"),
        }
        assert_eq!(
            store.load().expect("history remains"),
            AppStateFiles {
                binding: abandoned,
                recovery_ledger: RecoveryLedgerFile::empty(),
            }
        );
    }

    #[test]
    fn ledger_parse_accepts_empty_and_rejects_malformed_active() {
        assert!(
            RecoveryLedgerFile::parse(r#"{"schema_version": 1, "active": null, "completed": []}"#)
                .is_some()
        );
        let active = r#"{
            "schema_version": 1,
            "active": {
                "operation_id": "op-1",
                "kind": "fixture_recovery",
                "home_id": "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
                "created_at": "2026-08-01T00:00:00Z"
            },
            "completed": []
        }"#;
        assert!(RecoveryLedgerFile::parse(active).is_some());
        let bad = active.replace("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab", "not-a-uuid");
        assert!(RecoveryLedgerFile::parse(&bad).is_none());
        let bad_schema = active.replace("\"schema_version\": 1", "\"schema_version\": 9");
        assert!(RecoveryLedgerFile::parse(&bad_schema).is_none());
    }
}
