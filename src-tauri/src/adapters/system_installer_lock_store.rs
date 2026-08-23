//! System adapter for the installer lock seam: probes the known
//! `.skill-lock.json` locations (`~/.agents/.skill-lock.json` and the XDG
//! variant) and strictly parses every present file. Read-only; absent files
//! simply produce no report.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::seams::installer_lock_store::{
    InstallerLockError, InstallerLockStore, LockEntry, LockFileReport, LockReleaseError,
    parse_lock_bytes, release_lock_entries_bytes, release_lock_entry_bytes,
    restore_lock_entries_bytes, restore_lock_entry_bytes,
};

/// Default lock location: `~/.agents/.skill-lock.json`.
pub const DEFAULT_LOCK_RELATIVE_PATH: &str = ".agents/.skill-lock.json";
/// XDG variant: `$XDG_STATE_HOME/skills/.skill-lock.json`.
pub const XDG_LOCK_RELATIVE_PATH: &str = "skills/.skill-lock.json";

pub struct SystemInstallerLockStore {
    home_directory: PathBuf,
    /// `XDG_STATE_HOME` value; `None` means the XDG variant is not probed.
    xdg_state_home: Option<PathBuf>,
}

impl SystemInstallerLockStore {
    pub fn new(home_directory: PathBuf) -> Self {
        Self {
            home_directory,
            xdg_state_home: std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        }
    }

    /// Deterministic constructor for tests and compositions: pins the
    /// `XDG_STATE_HOME` probe instead of reading the environment.
    pub fn with_xdg(home_directory: PathBuf, xdg_state_home: Option<PathBuf>) -> Self {
        Self {
            home_directory,
            xdg_state_home,
        }
    }

    fn probe(path: &Path) -> Result<Option<LockFileReport>, InstallerLockError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(InstallerLockError::Io {
                    operation: "read installer lock",
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        Ok(Some(parse_lock_bytes(path, &bytes)))
    }
}

impl InstallerLockStore for SystemInstallerLockStore {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        let mut reports = Vec::new();
        let default = self.home_directory.join(DEFAULT_LOCK_RELATIVE_PATH);
        if let Some(report) = Self::probe(&default)? {
            reports.push(report);
        }
        if let Some(xdg) = &self.xdg_state_home {
            let xdg_lock = xdg.join(XDG_LOCK_RELATIVE_PATH);
            if let Some(report) = Self::probe(&xdg_lock)? {
                reports.push(report);
            }
        }
        Ok(reports)
    }

    fn release_entry(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
        entry: &LockEntry,
    ) -> Result<(), LockReleaseError> {
        let bytes = fs::read(lock_path).map_err(|source| {
            LockReleaseError::Io(format!("read {}: {source}", lock_path.display()))
        })?;
        let rewritten = release_lock_entry_bytes(&bytes, frozen_fingerprint, entry)?;
        write_lock_atomically(lock_path, &rewritten)
    }

    fn release_entries(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
        entries: &[LockEntry],
    ) -> Result<(), LockReleaseError> {
        let bytes = fs::read(lock_path).map_err(|source| {
            LockReleaseError::Io(format!("read {}: {source}", lock_path.display()))
        })?;
        let rewritten = release_lock_entries_bytes(&bytes, frozen_fingerprint, entries)?;
        write_lock_atomically(lock_path, &rewritten)
    }

    fn restore_entry(&self, lock_path: &Path, entry: &LockEntry) -> Result<(), LockReleaseError> {
        let bytes = match fs::read(lock_path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                // No lock file: restoring an entry means creating a minimal
                // valid v3 lock with exactly this entry.
                let value = serde_json::json!({
                    "version": 3,
                    "skills": { entry.name.clone(): serde_json::to_value(entry).map_err(|error| {
                        LockReleaseError::Invalid(error.to_string())
                    })? },
                });
                let bytes = serde_json::to_vec_pretty(&value)
                    .map_err(|error| LockReleaseError::Invalid(error.to_string()))?;
                return write_lock_atomically(lock_path, &bytes);
            }
            Err(source) => {
                return Err(LockReleaseError::Io(format!(
                    "read {}: {source}",
                    lock_path.display()
                )));
            }
        };
        let rewritten = restore_lock_entry_bytes(&bytes, entry)?;
        write_lock_atomically(lock_path, &rewritten)
    }

    fn restore_entries(
        &self,
        lock_path: &Path,
        entries: &[LockEntry],
    ) -> Result<(), LockReleaseError> {
        let bytes = fs::read(lock_path).map_err(|source| {
            LockReleaseError::Io(format!("read {}: {source}", lock_path.display()))
        })?;
        let rewritten = restore_lock_entries_bytes(&bytes, entries)?;
        write_lock_atomically(lock_path, &rewritten)
    }
}

/// The lock rewrite write protocol: temp file → full write → fsync →
/// atomic rename → parent fsync (spec §3.2 protocol, applied to the lock).
fn write_lock_atomically(lock_path: &Path, bytes: &[u8]) -> Result<(), LockReleaseError> {
    let parent = lock_path
        .parent()
        .ok_or_else(|| LockReleaseError::Io("the lock path has no parent".into()))?;
    let temporary = parent.join(".skill-lock.json.tmp");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|source| {
            LockReleaseError::Io(format!("open {}: {source}", temporary.display()))
        })?;
    file.write_all(bytes).map_err(|source| {
        LockReleaseError::Io(format!("write {}: {source}", temporary.display()))
    })?;
    file.sync_all().map_err(|source| {
        LockReleaseError::Io(format!("sync {}: {source}", temporary.display()))
    })?;
    fs::rename(&temporary, lock_path).map_err(|source| {
        LockReleaseError::Io(format!("rename {}: {source}", lock_path.display()))
    })?;
    let directory = fs::File::open(parent)
        .map_err(|source| LockReleaseError::Io(format!("open {}: {source}", parent.display())))?;
    directory
        .sync_all()
        .map_err(|source| LockReleaseError::Io(format!("sync {}: {source}", parent.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_nothing_when_no_lock_exists() {
        let root = tempfile::tempdir().expect("temporary home");
        let store = SystemInstallerLockStore::new(root.path().to_path_buf());
        assert!(store.discover().expect("discover").is_empty());
    }

    #[test]
    fn discover_parses_the_default_lock_location() {
        let root = tempfile::tempdir().expect("temporary home");
        let lock_path = root.path().join(DEFAULT_LOCK_RELATIVE_PATH);
        fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("create lock parent");
        fs::write(&lock_path, r#"{"version": 3, "skills": {}}"#).expect("write lock");
        let store = SystemInstallerLockStore::new(root.path().to_path_buf());
        let reports = store.discover().expect("discover");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].path, lock_path);
        assert_eq!(reports[0].version, 3);
    }

    #[test]
    fn discover_parses_the_xdg_variant_when_configured() {
        let root = tempfile::tempdir().expect("temporary home");
        let xdg = root.path().join("xdg-state");
        let lock_path = xdg.join(XDG_LOCK_RELATIVE_PATH);
        fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("create lock parent");
        fs::write(&lock_path, "not json").expect("write corrupt lock");
        let store = SystemInstallerLockStore::with_xdg(root.path().to_path_buf(), Some(xdg));
        let reports = store.discover().expect("discover");
        assert_eq!(reports.len(), 1);
        assert!(reports[0].fault.is_some());
    }

    #[test]
    fn discover_returns_both_locations_with_duplicate_declarations() {
        let root = tempfile::tempdir().expect("temporary home");
        let default = root.path().join(DEFAULT_LOCK_RELATIVE_PATH);
        let xdg = root.path().join("xdg-state");
        fs::create_dir_all(default.parent().expect("default parent"))
            .expect("create default parent");
        fs::create_dir_all(xdg.join("skills")).expect("create xdg skills");
        let valid_entry = r#""dupe": {
            "sourceType": "github",
            "source": "acme/dupe",
            "sourceUrl": "https://github.com/acme/dupe",
            "skillPath": "skills/dupe",
            "skillFolderHash": "0123456789abcdef0123456789abcdef01234567"
        }"#;
        fs::write(
            &default,
            format!(r#"{{"version": 3, "skills": {{{valid_entry}}}}}"#),
        )
        .expect("write default lock");
        fs::write(
            xdg.join("skills/.skill-lock.json"),
            format!(r#"{{"version": 3, "skills": {{{valid_entry}}}}}"#),
        )
        .expect("write xdg lock");
        let store = SystemInstallerLockStore::with_xdg(root.path().to_path_buf(), Some(xdg));
        let reports = store.discover().expect("discover");
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].entries.len(), 1);
        assert_eq!(reports[1].entries.len(), 1);
    }
}
