//! System `AppStateStore` adapter: reads and atomically writes the app-level
//! state files (`home-binding.json`, `recovery-ledger.json`) with the
//! tmp → full write → fsync → atomic rename → parent fsync protocol (§3.2).
//! A missing file is the empty state; an unreadable directory or an invalid
//! file is a hard error the bootstrap must surface, never guess.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use crate::seams::app_state_store::{
    AppStateFiles, AppStateStore, AppStateStoreError, HOME_BINDING_FILE_NAME, HomeBindingFile,
    RECOVERY_LEDGER_FILE_NAME, RecoveryLedgerFile,
};

pub struct AppStateStoreFileSystem {
    state_dir: PathBuf,
    state_guard: Mutex<()>,
}

impl AppStateStoreFileSystem {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            state_dir,
            state_guard: Mutex::new(()),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, ()>, AppStateStoreError> {
        self.state_guard
            .lock()
            .map_err(|_| AppStateStoreError::WriteFailed("app-state lock poisoned".into()))
    }

    fn read_or_empty<T>(
        &self,
        file_name: &str,
        parse: fn(&str) -> Option<T>,
        empty: T,
        invalid: fn(String) -> AppStateStoreError,
    ) -> Result<T, AppStateStoreError> {
        let path = self.state_dir.join(file_name);
        match fs::read_to_string(&path) {
            Ok(content) => parse(&content).ok_or_else(|| invalid(path.display().to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(empty),
            Err(error) => Err(AppStateStoreError::StateDirUnreadable(format!(
                "{}: {error}",
                path.display()
            ))),
        }
    }

    fn write_atomic(&self, file_name: &str, json: String) -> Result<(), AppStateStoreError> {
        write_atomic(&self.state_dir, file_name, json)
    }

    fn load_unlocked(&self) -> Result<AppStateFiles, AppStateStoreError> {
        if !self.state_dir.exists() {
            return Ok(AppStateFiles {
                binding: HomeBindingFile::empty(),
                recovery_ledger: RecoveryLedgerFile::empty(),
            });
        }
        if !self.state_dir.is_dir() {
            return Err(AppStateStoreError::StateDirUnreadable(format!(
                "{} is not a directory",
                self.state_dir.display()
            )));
        }
        let binding = self.read_or_empty(
            HOME_BINDING_FILE_NAME,
            HomeBindingFile::parse,
            HomeBindingFile::empty(),
            AppStateStoreError::LocatorInvalid,
        )?;
        let recovery_ledger = self.read_or_empty(
            RECOVERY_LEDGER_FILE_NAME,
            RecoveryLedgerFile::parse,
            RecoveryLedgerFile::empty(),
            AppStateStoreError::LedgerInvalid,
        )?;
        Ok(AppStateFiles {
            binding,
            recovery_ledger,
        })
    }

    fn write_locator_unlocked(&self, binding: &HomeBindingFile) -> Result<(), AppStateStoreError> {
        let json = serde_json::to_string_pretty(binding)
            .map_err(|error| AppStateStoreError::WriteFailed(error.to_string()))?;
        self.write_atomic(HOME_BINDING_FILE_NAME, json)
    }

    fn write_recovery_ledger_unlocked(
        &self,
        ledger: &RecoveryLedgerFile,
    ) -> Result<(), AppStateStoreError> {
        let json = serde_json::to_string_pretty(ledger)
            .map_err(|error| AppStateStoreError::WriteFailed(error.to_string()))?;
        self.write_atomic(RECOVERY_LEDGER_FILE_NAME, json)
    }
}

/// The tmp → full write → fsync → atomic rename → parent fsync protocol
/// (§3.2): a kill at any point leaves either the old file or the complete
/// new one, never a torn write. Shared by every app-level state file,
/// including `locale.json` (ADR-0011).
pub(crate) fn write_atomic(
    state_dir: &std::path::Path,
    file_name: &str,
    json: String,
) -> Result<(), AppStateStoreError> {
    let target = state_dir.join(file_name);
    let tmp = state_dir.join(format!("{file_name}.tmp"));
    fs::create_dir_all(state_dir).map_err(|error| {
        AppStateStoreError::WriteFailed(format!("{}: {error}", state_dir.display()))
    })?;
    let mut file = File::create(&tmp)
        .map_err(|error| AppStateStoreError::WriteFailed(format!("{tmp:?}: {error}")))?;
    file.write_all(json.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| AppStateStoreError::WriteFailed(format!("{tmp:?}: {error}")))?;
    drop(file);
    fs::rename(&tmp, &target).map_err(|error| {
        AppStateStoreError::WriteFailed(format!(
            "{} → {}: {error}",
            tmp.display(),
            target.display()
        ))
    })?;
    // Parent fsync makes the rename durable (the commit point).
    let dir = File::open(state_dir).map_err(|error| {
        AppStateStoreError::WriteFailed(format!("{}: {error}", state_dir.display()))
    })?;
    dir.sync_all().map_err(|error| {
        AppStateStoreError::WriteFailed(format!("{}: {error}", state_dir.display()))
    })?;
    Ok(())
}

impl AppStateStore for AppStateStoreFileSystem {
    fn load(&self) -> Result<AppStateFiles, AppStateStoreError> {
        let _guard = self.lock()?;
        self.load_unlocked()
    }

    fn write_locator(&self, binding: &HomeBindingFile) -> Result<(), AppStateStoreError> {
        let _guard = self.lock()?;
        self.write_locator_unlocked(binding)
    }

    fn cas_locator(
        &self,
        expected_current: Option<&crate::core::home::HomeId>,
        next: &HomeBindingFile,
    ) -> Result<(), AppStateStoreError> {
        let _guard = self.lock()?;
        let files = self.load_unlocked()?;
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
        self.write_locator_unlocked(next)
    }

    fn cas_unconfigured_locator(&self, next: &HomeBindingFile) -> Result<(), AppStateStoreError> {
        let _guard = self.lock()?;
        let files = self.load_unlocked()?;
        if files.binding.current.is_some()
            || !files.binding.abandoned.is_empty()
            || files.recovery_ledger.active.is_some()
        {
            return Err(AppStateStoreError::LocatorCasConflict {
                expected: None,
                found: files.binding.current.map(|current| current.home_id.0),
            });
        }
        self.write_locator_unlocked(next)
    }

    fn write_recovery_ledger(&self, ledger: &RecoveryLedgerFile) -> Result<(), AppStateStoreError> {
        let _guard = self.lock()?;
        self.write_recovery_ledger_unlocked(ledger)
    }
}

/// Crash-window safety proof: the protocol writes complete bytes, fsyncs the
/// file, renames atomically, then fsyncs the parent; a kill at any point
/// leaves either the old locator or the complete new one.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, mpsc};

    use crate::core::home::HomeId;
    use crate::seams::app_state_store::{AbandonedHomeRecord, HomeBindingRecord};

    use super::*;

    fn binding() -> HomeBindingFile {
        HomeBindingFile {
            schema_version: 1,
            current: Some(HomeBindingRecord {
                home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
                path: PathBuf::from("/tmp/skill-man-home"),
                volume_fsid: "fsid-1".into(),
                volume_uuid: "uuid-1".into(),
                bound_at: "2026-08-01T00:00:00Z".into(),
            }),
            abandoned: vec![],
        }
    }

    fn abandoned_binding() -> HomeBindingFile {
        HomeBindingFile {
            schema_version: 1,
            current: None,
            abandoned: vec![AbandonedHomeRecord {
                home_id: HomeId("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into()),
                path: PathBuf::from("/tmp/skill-man-home"),
                volume_fsid: "fsid-1".into(),
                volume_uuid: "uuid-1".into(),
                abandoned_at: "2026-08-01T00:00:00Z".into(),
            }],
        }
    }

    #[test]
    fn missing_state_dir_is_the_empty_fresh_state() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = AppStateStoreFileSystem::new(dir.path().join("state"));
        let files = store.load().expect("fresh state");
        assert_eq!(files.binding, HomeBindingFile::empty());
        assert_eq!(files.recovery_ledger, RecoveryLedgerFile::empty());
    }

    #[test]
    fn write_locator_is_atomic_and_readable_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = AppStateStoreFileSystem::new(dir.path().join("state"));
        store.write_locator(&binding()).expect("write locator");
        let files = store.load().expect("read back");
        assert_eq!(files.binding, binding());
        // The tmp file is gone; only the committed name exists.
        assert!(!dir.path().join("state/home-binding.json.tmp").exists());
        assert!(dir.path().join("state/home-binding.json").is_file());
    }

    #[test]
    fn unconfigured_cas_cannot_overwrite_history_written_by_a_waiting_contender() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(AppStateStoreFileSystem::new(dir.path().join("state")));
        let next = binding();

        // Keep the CAS's serialized critical section unavailable while the
        // contender begins. The history write then occurs before the CAS can
        // inspect state; it must see the completed transition and refuse it.
        let guard = store.lock().expect("hold app-state critical section");
        let (started_tx, started_rx) = mpsc::channel();
        let contender = {
            let store = store.clone();
            std::thread::spawn(move || {
                started_tx.send(()).expect("announce contender");
                store.cas_unconfigured_locator(&next)
            })
        };
        started_rx.recv().expect("contender started");
        let history = abandoned_binding();
        store
            .write_locator_unlocked(&history)
            .expect("commit concurrent history while lock is held");
        drop(guard);

        assert!(matches!(
            contender.join().expect("join contender"),
            Err(AppStateStoreError::LocatorCasConflict {
                expected: None,
                found: None,
            })
        ));
        assert_eq!(store.load().expect("history remains").binding, history);
    }

    #[test]
    fn invalid_locator_json_is_a_hard_error_not_a_guess() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).expect("state dir");
        std::fs::write(state.join("home-binding.json"), "{ not json").expect("corrupt locator");
        let store = AppStateStoreFileSystem::new(state);
        match store.load() {
            Err(AppStateStoreError::LocatorInvalid(_)) => {}
            other => panic!("expected LocatorInvalid, got {other:?}"),
        }
    }

    #[test]
    fn invalid_ledger_json_is_a_hard_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).expect("state dir");
        std::fs::write(state.join("recovery-ledger.json"), "[]").expect("invalid ledger");
        let store = AppStateStoreFileSystem::new(state);
        match store.load() {
            Err(AppStateStoreError::LedgerInvalid(_)) => {}
            other => panic!("expected LedgerInvalid, got {other:?}"),
        }
    }

    #[test]
    fn state_dir_that_is_a_file_is_unreadable() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = dir.path().join("state");
        std::fs::write(&state, "not a directory").expect("file in place");
        let store = AppStateStoreFileSystem::new(state);
        match store.load() {
            Err(AppStateStoreError::StateDirUnreadable(_)) => {}
            other => panic!("expected StateDirUnreadable, got {other:?}"),
        }
    }
}
