//! System adapter for the installer lock seam: probes the known
//! `.skill-lock.json` locations (`~/.agents/.skill-lock.json` and the XDG
//! variant), strictly parses every present file, and owns the safe full-file
//! CAS rewrite used by Source Transition. Absent files simply produce no
//! report.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use crate::seams::installer_lock_store::{
    InstallerLockError, InstallerLockStore, LockEntry, LockFileIdentity, LockFileReport,
    LockReleaseError, PendingLockCasState, parse_lock_bytes, release_lock_entries_bytes,
    restore_lock_entries_bytes, restore_lock_entry_bytes,
};

const LOCK_CAS_TEMP_NAME: &str = ".skill-lock.json.tmp";
const LOCK_CAS_MARKER_NAME: &str = ".skill-lock.json.cas";

/// Default lock location: `~/.agents/.skill-lock.json`.
pub const DEFAULT_LOCK_RELATIVE_PATH: &str = ".agents/.skill-lock.json";
/// XDG variant: `$XDG_STATE_HOME/skills/.skill-lock.json`.
pub const XDG_LOCK_RELATIVE_PATH: &str = "skills/.skill-lock.json";

pub struct SystemInstallerLockStore {
    home_directory: PathBuf,
    /// `XDG_STATE_HOME` value; `None` means the XDG variant is not probed.
    xdg_state_home: Option<PathBuf>,
    observed_identities: Mutex<HashMap<(PathBuf, String), LockFileIdentity>>,
}

impl SystemInstallerLockStore {
    pub fn new(home_directory: PathBuf) -> Self {
        Self {
            home_directory,
            xdg_state_home: std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
            observed_identities: Mutex::new(HashMap::new()),
        }
    }

    /// Deterministic constructor for tests and compositions: pins the
    /// `XDG_STATE_HOME` probe instead of reading the environment.
    pub fn with_xdg(home_directory: PathBuf, xdg_state_home: Option<PathBuf>) -> Self {
        Self {
            home_directory,
            xdg_state_home,
            observed_identities: Mutex::new(HashMap::new()),
        }
    }

    fn probe(
        path: &Path,
    ) -> Result<Option<(LockFileReport, LockFileIdentity)>, InstallerLockError> {
        let Some(snapshot) = read_lock_snapshot(path)? else {
            return Ok(None);
        };
        Ok(Some((
            parse_lock_bytes(path, &snapshot.bytes),
            snapshot.identity,
        )))
    }

    fn release_entries_impl(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
        frozen_identity: Option<&LockFileIdentity>,
        entries: &[LockEntry],
    ) -> Result<(), LockReleaseError> {
        let snapshot = read_lock_snapshot_for_release(lock_path)?;
        let expected_identity = frozen_identity
            .copied()
            .or_else(|| self.observed_identity(lock_path, frozen_fingerprint))
            .unwrap_or(snapshot.identity);
        if expected_identity != snapshot.identity {
            return Err(LockReleaseError::FingerprintChanged);
        }
        let rewritten = release_lock_entries_bytes(&snapshot.bytes, frozen_fingerprint, entries)?;
        write_lock_atomically(
            lock_path,
            &rewritten,
            Some(&expected_identity),
            Some(frozen_fingerprint),
        )
    }
}

impl InstallerLockStore for SystemInstallerLockStore {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        let mut reports = Vec::new();
        let default = self.home_directory.join(DEFAULT_LOCK_RELATIVE_PATH);
        if let Some((report, identity)) = Self::probe(&default)? {
            if let Ok(mut observed) = self.observed_identities.lock() {
                observed.insert((report.path.clone(), report.fingerprint.clone()), identity);
            }
            reports.push(report);
        }
        if let Some(xdg) = &self.xdg_state_home {
            let xdg_lock = xdg.join(XDG_LOCK_RELATIVE_PATH);
            if let Some((report, identity)) = Self::probe(&xdg_lock)? {
                if let Ok(mut observed) = self.observed_identities.lock() {
                    observed.insert((report.path.clone(), report.fingerprint.clone()), identity);
                }
                reports.push(report);
            }
        }
        Ok(reports)
    }

    fn observed_identity(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
    ) -> Option<LockFileIdentity> {
        self.observed_identities.lock().ok().and_then(|observed| {
            observed
                .get(&(lock_path.to_path_buf(), frozen_fingerprint.to_owned()))
                .copied()
        })
    }

    fn recover_pending_lock_cas(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
        frozen_identity: Option<&LockFileIdentity>,
        entries: &[LockEntry],
    ) -> Result<PendingLockCasState, LockReleaseError> {
        let (parent, final_name) = open_lock_parent(lock_path).map_err(|source| {
            LockReleaseError::Io(format!("open {}: {source}", lock_path.display()))
        })?;
        lock_parent_exclusively(&parent, lock_path)?;
        recover_pending_lock_cas_at(
            &parent,
            &final_name,
            lock_path,
            frozen_fingerprint,
            frozen_identity,
            entries,
        )
    }

    fn release_entry(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
        entry: &LockEntry,
    ) -> Result<(), LockReleaseError> {
        self.release_entries_impl(
            lock_path,
            frozen_fingerprint,
            None,
            std::slice::from_ref(entry),
        )
    }

    fn release_entries(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
        entries: &[LockEntry],
    ) -> Result<(), LockReleaseError> {
        self.release_entries_impl(lock_path, frozen_fingerprint, None, entries)
    }

    fn release_entries_with_identity(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
        frozen_identity: Option<&LockFileIdentity>,
        entries: &[LockEntry],
    ) -> Result<(), LockReleaseError> {
        self.release_entries_impl(lock_path, frozen_fingerprint, frozen_identity, entries)
    }

    fn restore_entry(&self, lock_path: &Path, entry: &LockEntry) -> Result<(), LockReleaseError> {
        let snapshot = match read_lock_snapshot(lock_path).map_err(|error| {
            LockReleaseError::Io(format!("read {}: {error}", lock_path.display()))
        })? {
            Some(snapshot) => Some(snapshot),
            None => {
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
                return write_lock_atomically(lock_path, &bytes, None, None);
            }
        };
        let snapshot = snapshot.expect("existing lock snapshot");
        let rewritten = restore_lock_entry_bytes(&snapshot.bytes, entry)?;
        let fingerprint = lock_fingerprint(&snapshot.bytes);
        write_lock_atomically(
            lock_path,
            &rewritten,
            Some(&snapshot.identity),
            Some(&fingerprint),
        )
    }

    fn restore_entries(
        &self,
        lock_path: &Path,
        entries: &[LockEntry],
    ) -> Result<(), LockReleaseError> {
        let snapshot = read_lock_snapshot_for_release(lock_path)?;
        let rewritten = restore_lock_entries_bytes(&snapshot.bytes, entries)?;
        let fingerprint = lock_fingerprint(&snapshot.bytes);
        write_lock_atomically(
            lock_path,
            &rewritten,
            Some(&snapshot.identity),
            Some(&fingerprint),
        )
    }
}

/// The lock rewrite protocol: an O_EXCL/O_NOFOLLOW temp file → full write →
/// fsync → identity-checked atomic exchange (or no-replace publish) →
/// parent fsync. The exchange keeps a concurrent replacement recoverable
/// instead of blindly renaming over it.
fn write_lock_atomically(
    lock_path: &Path,
    bytes: &[u8],
    expected_identity: Option<&LockFileIdentity>,
    expected_fingerprint: Option<&str>,
) -> Result<(), LockReleaseError> {
    let (parent, final_name) = open_lock_parent(lock_path).map_err(|source| {
        LockReleaseError::Io(format!("open {}: {source}", lock_path.display()))
    })?;
    // Serialize all Skill Man lock rewrites for this parent directory. The
    // lock file is externally owned, so the descriptor-relative exchange
    // below still validates bytes/inode after every exchange; this advisory
    // parent lock closes the check→exchange window for cooperating writers
    // instead of relying on an unlink/rename sequence.
    lock_parent_exclusively(&parent, lock_path)?;
    let temporary_name =
        CString::new(LOCK_CAS_TEMP_NAME).expect("static lock temp name has no NUL");
    let temporary_path = lock_path
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .join(LOCK_CAS_TEMP_NAME);
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            temporary_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(LockReleaseError::Io(format!(
            "open {}: {}",
            temporary_path.display(),
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let temporary_identity = match descriptor_identity(&descriptor, &temporary_path) {
        Ok(identity) => identity,
        Err(error) => {
            drop(descriptor);
            return Err(error);
        }
    };
    let mut file = fs::File::from(descriptor);
    if let Err(source) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = unlink_temp(
            &parent,
            &temporary_name,
            &temporary_path,
            &temporary_identity,
        );
        return Err(LockReleaseError::Io(format!(
            "write {}: {source}",
            temporary_path.display()
        )));
    }
    drop(file);
    let marker_name = CString::new(LOCK_CAS_MARKER_NAME).expect("static marker name has no NUL");
    let marker_path = lock_path
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .join(LOCK_CAS_MARKER_NAME);
    let marker_identity = match write_cas_marker(
        &parent,
        &marker_name,
        &marker_path,
        lock_path,
        expected_identity,
        expected_fingerprint,
        &temporary_identity,
        bytes,
    ) {
        Ok(identity) => identity,
        Err(error) => {
            let _ = unlink_temp(
                &parent,
                &temporary_name,
                &temporary_path,
                &temporary_identity,
            );
            return Err(error);
        }
    };

    if let Some(expected_identity) = expected_identity {
        let current_metadata = metadata_at_nofollow(&parent, &final_name, lock_path)
            .map_err(|error| LockReleaseError::Io(error.to_string()))?;
        let current_identity = LockFileIdentity {
            device: current_metadata.st_dev as u64,
            inode: current_metadata.st_ino,
        };
        if current_identity != *expected_identity {
            unlink_temp(
                &parent,
                &temporary_name,
                &temporary_path,
                &temporary_identity,
            )?;
            clear_cas_marker(&parent, &marker_name, &marker_path, &marker_identity)?;
            return Err(LockReleaseError::FingerprintChanged);
        }
        let status = unsafe {
            libc::renameatx_np(
                parent.as_raw_fd(),
                temporary_name.as_ptr(),
                parent.as_raw_fd(),
                final_name.as_ptr(),
                libc::RENAME_SWAP,
            )
        };
        if status != 0 {
            let source = std::io::Error::last_os_error();
            unlink_temp(
                &parent,
                &temporary_name,
                &temporary_path,
                &temporary_identity,
            )?;
            clear_cas_marker(&parent, &marker_name, &marker_path, &marker_identity)?;
            return if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::AlreadyExists
            ) {
                Err(LockReleaseError::FingerprintChanged)
            } else {
                Err(LockReleaseError::Io(format!(
                    "swap {}: {source}",
                    lock_path.display()
                )))
            };
        }

        let previous = read_regular_at(&parent, &temporary_name, &temporary_path);
        let previous_snapshot = match previous {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) | Err(_) => {
                return Err(LockReleaseError::RecoveryRequired(format!(
                    "the pre-CAS lock claim is not a readable regular file: {}",
                    lock_path.display()
                )));
            }
        };
        let matches_frozen = previous_snapshot.identity == *expected_identity
            && expected_fingerprint
                .is_some_and(|expected| lock_fingerprint(&previous_snapshot.bytes) == expected);
        if matches_frozen {
            unlink_temp(
                &parent,
                &temporary_name,
                &temporary_path,
                &previous_snapshot.identity,
            )?;
            sync_lock_parent(&parent, lock_path)?;
            clear_cas_marker(&parent, &marker_name, &marker_path, &marker_identity)?;
            return Ok(());
        }

        // The exchange observed a different inode or different bytes. Chase
        // the external claim through a bounded sequence of post-checked
        // exchanges. Every exchange is followed by identity checks; a claim
        // is never unlinked unless the temporary slot is the exact displaced
        // file we intended to discard.
        let mut expected_final: Option<LockFileSnapshot> = None;
        for _ in 0..8 {
            let final_snapshot = read_regular_at(&parent, &final_name, lock_path)
                .ok()
                .and_then(|snapshot| snapshot);
            let temporary_snapshot = read_regular_at(&parent, &temporary_name, &temporary_path)
                .ok()
                .and_then(|snapshot| snapshot);
            if temporary_snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.identity == temporary_identity && snapshot.bytes == bytes
            }) {
                unlink_temp(
                    &parent,
                    &temporary_name,
                    &temporary_path,
                    &temporary_identity,
                )?;
                sync_lock_parent(&parent, lock_path)?;
                clear_cas_marker(&parent, &marker_name, &marker_path, &marker_identity)?;
                return Err(LockReleaseError::FingerprintChanged);
            }

            let final_is_our_temp = final_snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.identity == temporary_identity && snapshot.bytes == bytes
            });
            if final_is_our_temp {
                if temporary_snapshot.is_none() {
                    return Err(LockReleaseError::RecoveryRequired(format!(
                        "the lock compensation slot vanished: {}",
                        lock_path.display()
                    )));
                }
            } else if let Some(expected) = &expected_final {
                if final_snapshot.as_ref() != Some(expected)
                    || temporary_snapshot.is_none()
                    || temporary_snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.identity == temporary_identity && snapshot.bytes == bytes
                    })
                {
                    return Err(LockReleaseError::RecoveryRequired(format!(
                        "the lock changed while the CAS rollback was in progress: {}",
                        lock_path.display()
                    )));
                }
            } else {
                return Err(LockReleaseError::RecoveryRequired(format!(
                    "the lock changed while the CAS rollback was in progress: {}",
                    lock_path.display()
                )));
            }

            let restore_status = unsafe {
                libc::renameatx_np(
                    parent.as_raw_fd(),
                    temporary_name.as_ptr(),
                    parent.as_raw_fd(),
                    final_name.as_ptr(),
                    libc::RENAME_SWAP,
                )
            };
            if restore_status != 0 {
                return Err(LockReleaseError::RecoveryRequired(format!(
                    "restore concurrent lock claim {}: {}",
                    lock_path.display(),
                    std::io::Error::last_os_error()
                )));
            }
            let after_temp = read_regular_at(&parent, &temporary_name, &temporary_path)
                .ok()
                .and_then(|snapshot| snapshot);
            let after_final = read_regular_at(&parent, &final_name, lock_path)
                .ok()
                .and_then(|snapshot| snapshot);
            if after_temp.as_ref().is_some_and(|snapshot| {
                snapshot.identity == temporary_identity && snapshot.bytes == bytes
            }) {
                unlink_temp(
                    &parent,
                    &temporary_name,
                    &temporary_path,
                    &temporary_identity,
                )?;
                sync_lock_parent(&parent, lock_path)?;
                clear_cas_marker(&parent, &marker_name, &marker_path, &marker_identity)?;
                return Err(LockReleaseError::FingerprintChanged);
            }
            expected_final = after_final;
        }
        Err(LockReleaseError::RecoveryRequired(format!(
            "the lock changed repeatedly while the CAS rollback was in progress: {}",
            lock_path.display()
        )))
    } else {
        let status = unsafe {
            libc::renameatx_np(
                parent.as_raw_fd(),
                temporary_name.as_ptr(),
                parent.as_raw_fd(),
                final_name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if status != 0 {
            let source = std::io::Error::last_os_error();
            unlink_temp(
                &parent,
                &temporary_name,
                &temporary_path,
                &temporary_identity,
            )?;
            clear_cas_marker(&parent, &marker_name, &marker_path, &marker_identity)?;
            return Err(LockReleaseError::Io(format!(
                "publish {}: {source}",
                lock_path.display()
            )));
        }
        sync_lock_parent(&parent, lock_path)?;
        clear_cas_marker(&parent, &marker_name, &marker_path, &marker_identity)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LockFileSnapshot {
    bytes: Vec<u8>,
    identity: LockFileIdentity,
}

fn lock_fingerprint(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[allow(clippy::too_many_arguments)]
fn write_cas_marker(
    parent: &OwnedFd,
    marker_name: &CString,
    marker_path: &Path,
    lock_path: &Path,
    expected_identity: Option<&LockFileIdentity>,
    expected_fingerprint: Option<&str>,
    rewritten_identity: &LockFileIdentity,
    rewritten_bytes: &[u8],
) -> Result<LockFileIdentity, LockReleaseError> {
    let marker = serde_json::json!({
        "version": 1,
        "temporaryName": LOCK_CAS_TEMP_NAME,
        "expectedFingerprint": expected_fingerprint,
        "expectedIdentity": expected_identity.map(|identity| serde_json::json!({
            "device": identity.device,
            "inode": identity.inode,
        })),
        "rewrittenFingerprint": lock_fingerprint(rewritten_bytes),
        "rewrittenIdentity": {
            "device": rewritten_identity.device,
            "inode": rewritten_identity.inode,
        },
    });
    let bytes = serde_json::to_vec(&marker).map_err(|error| {
        LockReleaseError::Invalid(format!("serialize lock CAS marker: {error}"))
    })?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            marker_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(LockReleaseError::Io(format!(
            "open CAS marker {} for {}: {}",
            marker_path.display(),
            lock_path.display(),
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let identity = descriptor_identity(&descriptor, marker_path)?;
    let mut marker_file = fs::File::from(descriptor);
    marker_file
        .write_all(&bytes)
        .and_then(|()| marker_file.sync_all())
        .map_err(|source| {
            LockReleaseError::Io(format!(
                "write CAS marker {}: {source}",
                marker_path.display()
            ))
        })?;
    drop(marker_file);
    sync_lock_parent(parent, lock_path)?;
    Ok(identity)
}

fn clear_cas_marker(
    parent: &OwnedFd,
    marker_name: &CString,
    marker_path: &Path,
    marker_identity: &LockFileIdentity,
) -> Result<(), LockReleaseError> {
    unlink_temp(parent, marker_name, marker_path, marker_identity).map_err(|error| {
        LockReleaseError::RecoveryRequired(format!(
            "remove lock CAS marker {}: {error}",
            marker_path.display()
        ))
    })?;
    sync_lock_parent(parent, marker_path).map_err(|error| {
        LockReleaseError::RecoveryRequired(format!(
            "sync lock CAS marker parent {}: {error}",
            marker_path.display()
        ))
    })
}

fn read_lock_snapshot(path: &Path) -> Result<Option<LockFileSnapshot>, InstallerLockError> {
    let (parent, name) = match open_lock_parent(path) {
        Ok(value) => value,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(InstallerLockError::Io {
                operation: "open installer lock parent",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    lock_parent_shared(&parent, path)?;
    read_regular_at(&parent, &name, path).map_err(|source| InstallerLockError::Io {
        operation: "read installer lock",
        path: path.to_path_buf(),
        source,
    })
}

fn read_lock_snapshot_for_release(path: &Path) -> Result<LockFileSnapshot, LockReleaseError> {
    read_lock_snapshot(path)
        .map_err(|error| LockReleaseError::Io(error.to_string()))?
        .ok_or_else(|| LockReleaseError::Io(format!("lock file {} is absent", path.display())))
}

fn open_lock_parent(path: &Path) -> Result<(OwnedFd, CString), std::io::Error> {
    let parent_path = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "lock has no parent")
    })?;
    let parent_path = parent_path.canonicalize()?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "lock has no name"))?;
    let encoded_name = CString::new(name.as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "lock name contains NUL")
    })?;
    Ok((
        open_absolute_directory_chain_nofollow(&parent_path)?,
        encoded_name,
    ))
}

fn open_absolute_directory_chain_nofollow(path: &Path) -> Result<OwnedFd, std::io::Error> {
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "lock parent must be absolute",
        ));
    }
    let root = CString::new("/").expect("root has no NUL");
    let descriptor = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `open` returned a new owned descriptor.
    let mut current = unsafe { OwnedFd::from_raw_fd(descriptor) };
    for component in path.components() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::RootDir) {
                continue;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "lock parent contains an unsafe path component",
            ));
        };
        let encoded = CString::new(name.as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "lock parent contains a NUL component",
            )
        })?;
        let descriptor = unsafe {
            libc::openat(
                current.as_raw_fd(),
                encoded.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a new owned descriptor.
        current = unsafe { OwnedFd::from_raw_fd(descriptor) };
    }
    Ok(current)
}

fn descriptor_identity(
    descriptor: &OwnedFd,
    path: &Path,
) -> Result<LockFileIdentity, LockReleaseError> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(descriptor.as_raw_fd(), metadata.as_mut_ptr()) } != 0 {
        return Err(LockReleaseError::Io(format!(
            "inspect {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: a zero `fstat` return initialized the output.
    let metadata = unsafe { metadata.assume_init() };
    Ok(LockFileIdentity {
        device: metadata.st_dev as u64,
        inode: metadata.st_ino,
    })
}

fn metadata_at_nofollow(
    parent: &OwnedFd,
    name: &CString,
    path: &Path,
) -> Result<libc::stat, std::io::Error> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } == 0
    {
        // SAFETY: a zero `fstatat` return initialized the output.
        Ok(unsafe { metadata.assume_init() })
    } else {
        let _ = path;
        Err(std::io::Error::last_os_error())
    }
}

fn read_regular_at(
    parent: &OwnedFd,
    name: &CString,
    path: &Path,
) -> Result<Option<LockFileSnapshot>, std::io::Error> {
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let source = std::io::Error::last_os_error();
        if source.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(source);
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let identity = {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(descriptor.as_raw_fd(), metadata.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a zero `fstat` return initialized the output.
        let metadata = unsafe { metadata.assume_init() };
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a regular file", path.display()),
            ));
        }
        LockFileIdentity {
            device: metadata.st_dev as u64,
            inode: metadata.st_ino,
        }
    };
    let mut file = fs::File::from(descriptor);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(LockFileSnapshot { bytes, identity }))
}

fn lock_snapshot_has_claims(
    snapshot: &LockFileSnapshot,
    frozen_fingerprint: &str,
    frozen_identity: Option<&LockFileIdentity>,
    entries: &[LockEntry],
) -> bool {
    snapshot
        .identity
        .eq(frozen_identity.unwrap_or(&snapshot.identity))
        && lock_fingerprint(&snapshot.bytes) == frozen_fingerprint
        && !entries.is_empty()
        && release_lock_entries_bytes(&snapshot.bytes, frozen_fingerprint, entries).is_ok()
}

fn lock_snapshot_matches_marker(
    snapshot: Option<&LockFileSnapshot>,
    fingerprint: &str,
    identity: &LockFileIdentity,
) -> bool {
    snapshot.is_some_and(|snapshot| {
        lock_fingerprint(&snapshot.bytes) == fingerprint && snapshot.identity == *identity
    })
}

fn marker_identity(
    value: &serde_json::Value,
    key: &str,
) -> Result<LockFileIdentity, LockReleaseError> {
    let object = value
        .get(key)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| LockReleaseError::Invalid(format!("lock CAS marker lacks {key}")))?;
    Ok(LockFileIdentity {
        device: object
            .get("device")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                LockReleaseError::Invalid(format!("lock CAS marker lacks {key}.device"))
            })?,
        inode: object
            .get("inode")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                LockReleaseError::Invalid(format!("lock CAS marker lacks {key}.inode"))
            })?,
    })
}

fn recover_pending_lock_cas_at(
    parent: &OwnedFd,
    final_name: &CString,
    lock_path: &Path,
    frozen_fingerprint: &str,
    frozen_identity: Option<&LockFileIdentity>,
    entries: &[LockEntry],
) -> Result<PendingLockCasState, LockReleaseError> {
    let marker_name = CString::new(LOCK_CAS_MARKER_NAME).expect("static marker name has no NUL");
    let temporary_name = CString::new(LOCK_CAS_TEMP_NAME).expect("static temp name has no NUL");
    let marker_path = lock_path
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .join(LOCK_CAS_MARKER_NAME);
    let Some(marker_snapshot) =
        read_regular_at(parent, &marker_name, &marker_path).map_err(|source| {
            LockReleaseError::Io(format!("read {}: {source}", marker_path.display()))
        })?
    else {
        return Ok(PendingLockCasState::None);
    };
    let marker: serde_json::Value = serde_json::from_slice(&marker_snapshot.bytes)
        .map_err(|error| LockReleaseError::Invalid(format!("parse lock CAS marker: {error}")))?;
    if marker.get("version").and_then(serde_json::Value::as_u64) != Some(1)
        || marker
            .get("temporaryName")
            .and_then(serde_json::Value::as_str)
            != Some(LOCK_CAS_TEMP_NAME)
    {
        return Err(LockReleaseError::Invalid(
            "the lock CAS marker has an unsupported shape".into(),
        ));
    }
    let marker_fingerprint = marker
        .get("expectedFingerprint")
        .and_then(serde_json::Value::as_str);
    if marker_fingerprint != Some(frozen_fingerprint) {
        return Err(LockReleaseError::RecoveryRequired(
            "the pending lock CAS belongs to a different frozen fingerprint".into(),
        ));
    }
    let marker_expected_identity = marker
        .get("expectedIdentity")
        .filter(|value| !value.is_null())
        .map(|_| marker_identity(&marker, "expectedIdentity"))
        .transpose()?;
    if marker_expected_identity.as_ref() != frozen_identity {
        return Err(LockReleaseError::RecoveryRequired(
            "the pending lock CAS belongs to a different frozen identity".into(),
        ));
    }
    let rewritten_fingerprint = marker
        .get("rewrittenFingerprint")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            LockReleaseError::Invalid("lock CAS marker lacks rewrittenFingerprint".into())
        })?;
    let rewritten_identity = marker_identity(&marker, "rewrittenIdentity")?;
    let temporary_path = lock_path
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .join(LOCK_CAS_TEMP_NAME);
    let final_snapshot = read_regular_at(parent, final_name, lock_path).map_err(|source| {
        LockReleaseError::Io(format!("read {}: {source}", lock_path.display()))
    })?;
    let temporary_snapshot =
        read_regular_at(parent, &temporary_name, &temporary_path).map_err(|source| {
            LockReleaseError::Io(format!("read {}: {source}", temporary_path.display()))
        })?;
    let final_is_rewritten = lock_snapshot_matches_marker(
        final_snapshot.as_ref(),
        rewritten_fingerprint,
        &rewritten_identity,
    );
    let final_is_old = final_snapshot.as_ref().is_some_and(|snapshot| {
        lock_snapshot_has_claims(snapshot, frozen_fingerprint, frozen_identity, entries)
    });
    let temporary_is_rewritten = lock_snapshot_matches_marker(
        temporary_snapshot.as_ref(),
        rewritten_fingerprint,
        &rewritten_identity,
    );
    let temporary_is_old = temporary_snapshot.as_ref().is_some_and(|snapshot| {
        lock_snapshot_has_claims(snapshot, frozen_fingerprint, frozen_identity, entries)
    });

    if temporary_is_old {
        let old_identity = temporary_snapshot
            .as_ref()
            .expect("temporary_is_old implies a snapshot")
            .identity;
        if final_is_rewritten {
            unlink_temp(parent, &temporary_name, &temporary_path, &old_identity)?;
            clear_cas_marker(
                parent,
                &marker_name,
                &marker_path,
                &marker_snapshot.identity,
            )?;
            return Ok(PendingLockCasState::Released);
        }
        if final_is_old {
            unlink_temp(parent, &temporary_name, &temporary_path, &old_identity)?;
            clear_cas_marker(
                parent,
                &marker_name,
                &marker_path,
                &marker_snapshot.identity,
            )?;
            return Ok(PendingLockCasState::None);
        }
        return Err(LockReleaseError::RecoveryRequired(
            "the pending lock CAS has an unrecognized final lock state".into(),
        ));
    }

    if temporary_is_rewritten {
        if final_is_old {
            unlink_temp(
                parent,
                &temporary_name,
                &temporary_path,
                &rewritten_identity,
            )?;
            clear_cas_marker(
                parent,
                &marker_name,
                &marker_path,
                &marker_snapshot.identity,
            )?;
            return Ok(PendingLockCasState::None);
        }
        if final_snapshot.is_some() && !final_is_rewritten {
            unlink_temp(
                parent,
                &temporary_name,
                &temporary_path,
                &rewritten_identity,
            )?;
            clear_cas_marker(
                parent,
                &marker_name,
                &marker_path,
                &marker_snapshot.identity,
            )?;
            return Ok(PendingLockCasState::Present);
        }
        return Err(LockReleaseError::RecoveryRequired(
            "the pending lock CAS lost its live lock path".into(),
        ));
    }

    if final_is_rewritten {
        let status = unsafe {
            libc::renameatx_np(
                parent.as_raw_fd(),
                temporary_name.as_ptr(),
                parent.as_raw_fd(),
                final_name.as_ptr(),
                libc::RENAME_SWAP,
            )
        };
        if status != 0 {
            return Err(LockReleaseError::RecoveryRequired(format!(
                "restore pending external lock claim {}: {}",
                lock_path.display(),
                std::io::Error::last_os_error()
            )));
        }
        let restored =
            read_regular_at(parent, &temporary_name, &temporary_path).map_err(|source| {
                LockReleaseError::RecoveryRequired(format!(
                    "read restored lock CAS temporary {}: {source}",
                    temporary_path.display()
                ))
            })?;
        if !lock_snapshot_matches_marker(
            restored.as_ref(),
            rewritten_fingerprint,
            &rewritten_identity,
        ) {
            return Err(LockReleaseError::RecoveryRequired(
                "the pending lock CAS could not re-identify its rewritten bytes".into(),
            ));
        }
        unlink_temp(
            parent,
            &temporary_name,
            &temporary_path,
            &rewritten_identity,
        )?;
        clear_cas_marker(
            parent,
            &marker_name,
            &marker_path,
            &marker_snapshot.identity,
        )?;
        return Ok(PendingLockCasState::Present);
    }

    if final_is_old && temporary_snapshot.is_some() {
        return Err(LockReleaseError::RecoveryRequired(
            "the pending lock CAS has an external temporary claim beside the old lock".into(),
        ));
    }
    Err(LockReleaseError::RecoveryRequired(
        "the pending lock CAS has ambiguous final and temporary entries".into(),
    ))
}

fn unlink_temp(
    parent: &OwnedFd,
    name: &CString,
    path: &Path,
    expected: &LockFileIdentity,
) -> Result<(), LockReleaseError> {
    let metadata = metadata_at_nofollow(parent, name, path)
        .map_err(|source| LockReleaseError::Io(format!("inspect {}: {source}", path.display())))?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || metadata.st_dev as u64 != expected.device
        || metadata.st_ino != expected.inode
    {
        return Err(LockReleaseError::Io(format!(
            "temporary lock path {} changed identity",
            path.display()
        )));
    }
    let status = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if status != 0 {
        return Err(LockReleaseError::Io(format!(
            "remove {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn sync_lock_parent(parent: &OwnedFd, lock_path: &Path) -> Result<(), LockReleaseError> {
    if unsafe { libc::fsync(parent.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(LockReleaseError::Io(format!(
            "sync lock parent for {}: {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        )))
    }
}

fn lock_parent_exclusively(parent: &OwnedFd, lock_path: &Path) -> Result<(), LockReleaseError> {
    let status = unsafe { libc::flock(parent.as_raw_fd(), libc::LOCK_EX) };
    if status == 0 {
        Ok(())
    } else {
        Err(LockReleaseError::Io(format!(
            "lock parent for {}: {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        )))
    }
}

fn lock_parent_shared(parent: &OwnedFd, lock_path: &Path) -> Result<(), InstallerLockError> {
    let status = unsafe { libc::flock(parent.as_raw_fd(), libc::LOCK_SH) };
    if status == 0 {
        Ok(())
    } else {
        Err(InstallerLockError::Io {
            operation: "lock installer lock parent for read",
            path: lock_path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seams::installer_lock_store::release_lock_entry_bytes;
    use std::os::unix::fs::MetadataExt;

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

    #[test]
    fn release_rejects_a_byte_identical_lock_replacement_by_identity() {
        let root = tempfile::tempdir().expect("temporary home");
        let lock_path = root.path().join(DEFAULT_LOCK_RELATIVE_PATH);
        fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("create lock parent");
        let bytes = br#"{"version":3,"skills":{"dupe":{"sourceType":"github","source":"acme/dupe","sourceUrl":"https://github.com/acme/dupe","skillPath":"skills/dupe","skillFolderHash":"0123456789abcdef0123456789abcdef01234567"}}}"#;
        fs::write(&lock_path, bytes).expect("write original lock");
        let store = SystemInstallerLockStore::new(root.path().to_path_buf());
        let report = store
            .discover()
            .expect("discover")
            .into_iter()
            .next()
            .expect("lock report");
        let replacement = root.path().join("replacement-lock");
        fs::write(&replacement, bytes).expect("write byte-identical replacement");
        fs::rename(&replacement, &lock_path).expect("replace lock inode");

        let result = store.release_entry(&lock_path, &report.fingerprint, &report.entries[0]);

        assert!(matches!(result, Err(LockReleaseError::FingerprintChanged)));
        assert_eq!(fs::read(&lock_path).expect("read lock"), bytes);
    }

    #[test]
    fn release_rejects_lock_bytes_changed_after_discovery_without_writing() {
        let root = tempfile::tempdir().expect("temporary home");
        let lock_path = root.path().join(DEFAULT_LOCK_RELATIVE_PATH);
        fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("create lock parent");
        let original = br#"{"version":3,"skills":{"dupe":{"sourceType":"github","source":"acme/dupe","sourceUrl":"https://github.com/acme/dupe","skillPath":"skills/dupe","skillFolderHash":"0123456789abcdef0123456789abcdef01234567"}}}"#;
        let changed = br#"{"version":3,"skills":{"other":{"sourceType":"github","source":"acme/other","sourceUrl":"https://github.com/acme/other","skillPath":"skills/other","skillFolderHash":"0123456789abcdef0123456789abcdef01234567"}}}"#;
        fs::write(&lock_path, original).expect("write original lock");
        let store = SystemInstallerLockStore::new(root.path().to_path_buf());
        let report = store
            .discover()
            .expect("discover")
            .into_iter()
            .next()
            .expect("lock report");
        fs::write(&lock_path, changed).expect("modify lock bytes");

        let result = store.release_entry(&lock_path, &report.fingerprint, &report.entries[0]);

        assert!(matches!(result, Err(LockReleaseError::FingerprintChanged)));
        assert_eq!(fs::read(&lock_path).expect("read lock"), changed);
    }

    #[test]
    fn recovery_finishes_a_crashed_successful_lock_exchange() {
        let root = tempfile::tempdir().expect("temporary home");
        let lock_path = root.path().join(DEFAULT_LOCK_RELATIVE_PATH);
        fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("create lock parent");
        let original = br#"{"version":3,"skills":{"dupe":{"sourceType":"github","source":"acme/dupe","sourceUrl":"https://github.com/acme/dupe","skillPath":"skills/dupe","skillFolderHash":"0123456789abcdef0123456789abcdef01234567"}}}"#;
        fs::write(&lock_path, original).expect("write original lock");
        let store = SystemInstallerLockStore::new(root.path().to_path_buf());
        let report = store
            .discover()
            .expect("discover")
            .into_iter()
            .next()
            .expect("lock report");
        let old_identity = store
            .observed_identity(&lock_path, &report.fingerprint)
            .expect("old lock identity");
        let rewritten = release_lock_entry_bytes(original, &report.fingerprint, &report.entries[0])
            .expect("rewrite lock bytes");
        let temporary = lock_path
            .parent()
            .expect("lock parent")
            .join(LOCK_CAS_TEMP_NAME);
        fs::write(&temporary, &rewritten).expect("write rewritten temporary");
        let rewritten_metadata = fs::symlink_metadata(&temporary).expect("rewritten metadata");
        let rewritten_identity = LockFileIdentity {
            device: rewritten_metadata.dev(),
            inode: rewritten_metadata.ino(),
        };
        let backup = root.path().join("original-lock");
        fs::rename(&lock_path, &backup).expect("move original lock aside");
        fs::rename(&temporary, &lock_path).expect("publish rewritten lock");
        fs::rename(&backup, &temporary).expect("leave old lock in CAS slot");
        let (parent, _) = open_lock_parent(&lock_path).expect("open lock parent");
        let marker_name = CString::new(LOCK_CAS_MARKER_NAME).expect("marker name");
        let marker_path = lock_path.parent().unwrap().join(LOCK_CAS_MARKER_NAME);
        let marker_identity = write_cas_marker(
            &parent,
            &marker_name,
            &marker_path,
            &lock_path,
            Some(&old_identity),
            Some(&report.fingerprint),
            &rewritten_identity,
            &rewritten,
        )
        .expect("write CAS marker");
        drop(parent);
        assert_ne!(marker_identity.inode, 0);

        let state = store
            .recover_pending_lock_cas(
                &lock_path,
                &report.fingerprint,
                Some(&old_identity),
                &report.entries,
            )
            .expect("recover pending CAS");

        assert_eq!(state, PendingLockCasState::Released);
        assert_eq!(
            fs::read(&lock_path).expect("read recovered lock"),
            rewritten
        );
        assert!(!temporary.exists(), "recovered CAS temp must be removed");
        assert!(
            !marker_path.exists(),
            "recovered CAS marker must be removed"
        );
    }

    #[test]
    fn recovery_restores_an_external_claim_from_a_crashed_lock_exchange() {
        let root = tempfile::tempdir().expect("temporary home");
        let lock_path = root.path().join(DEFAULT_LOCK_RELATIVE_PATH);
        fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("create lock parent");
        let original = br#"{"version":3,"skills":{"dupe":{"sourceType":"github","source":"acme/dupe","sourceUrl":"https://github.com/acme/dupe","skillPath":"skills/dupe","skillFolderHash":"0123456789abcdef0123456789abcdef01234567"}}}"#;
        let external = br#"{"version":3,"skills":{"new-claim":{"sourceType":"github","source":"acme/new-claim","sourceUrl":"https://github.com/acme/new-claim","skillPath":"skills/new-claim","skillFolderHash":"0123456789abcdef0123456789abcdef01234567"}}}"#;
        fs::write(&lock_path, original).expect("write original lock");
        let store = SystemInstallerLockStore::new(root.path().to_path_buf());
        let report = store
            .discover()
            .expect("discover")
            .into_iter()
            .next()
            .expect("lock report");
        let old_identity = store
            .observed_identity(&lock_path, &report.fingerprint)
            .expect("old lock identity");
        let rewritten = release_lock_entry_bytes(original, &report.fingerprint, &report.entries[0])
            .expect("rewrite lock bytes");
        let temporary = lock_path.parent().unwrap().join(LOCK_CAS_TEMP_NAME);
        fs::write(&temporary, external).expect("write external temporary claim");
        let external_metadata = fs::symlink_metadata(&temporary).expect("external metadata");
        let external_identity = LockFileIdentity {
            device: external_metadata.dev(),
            inode: external_metadata.ino(),
        };
        let backup = root.path().join("original-lock");
        fs::rename(&lock_path, &backup).expect("move original lock aside");
        fs::write(&lock_path, &rewritten).expect("leave rewritten lock live");
        let (parent, _) = open_lock_parent(&lock_path).expect("open lock parent");
        let marker_name = CString::new(LOCK_CAS_MARKER_NAME).expect("marker name");
        let marker_path = lock_path.parent().unwrap().join(LOCK_CAS_MARKER_NAME);
        let _marker_identity = write_cas_marker(
            &parent,
            &marker_name,
            &marker_path,
            &lock_path,
            Some(&old_identity),
            Some(&report.fingerprint),
            &LockFileIdentity {
                device: fs::symlink_metadata(&lock_path).unwrap().dev(),
                inode: fs::symlink_metadata(&lock_path).unwrap().ino(),
            },
            &rewritten,
        )
        .expect("write CAS marker");
        drop(parent);

        // The temporary file is the external claim and the public path is
        // the frozen rewrite: exactly the ambiguous post-exchange state.
        let state = store
            .recover_pending_lock_cas(
                &lock_path,
                &report.fingerprint,
                Some(&old_identity),
                &report.entries,
            )
            .expect("recover external claim");

        assert_eq!(state, PendingLockCasState::Present);
        assert_eq!(
            fs::read(&lock_path).expect("read restored external lock"),
            external
        );
        assert!(!temporary.exists());
        assert!(!marker_path.exists());
        assert_ne!(external_identity.inode, 0);
    }
}
